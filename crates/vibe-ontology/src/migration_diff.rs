//! Migration diff generation for ontology package data.
//!
//! Rust port of `src/lib/migration-diff.mjs`, so the CLI, CI
//! (one-2xg.18), and the app share one implementation
//! (one-2xg.16 / one-2xg.46). Generated TypeQL, JSON ordering, and migration
//! paths intentionally match the JavaScript implementation.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::executable_package::{
    hash_bytes, split_put_statements_on_lf as split_put_statements, split_write_text,
};
use crate::upgrade_limits::{check_upgrade_unit, pipeline_stages, split_functions, SchemaKeys};

fn invalid<S: Into<String>>(message: S) -> Error {
    Error::Version(message.into())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PutGroup {
    variable: Option<String>,
    type_name: Option<String>,
    statements: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HasClause {
    attribute: String,
    value: String,
    annotation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AttributeChange {
    attribute: String,
    old_values: Vec<String>,
    new_values: Vec<String>,
}

fn put_entity_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"^put\s+\$((?-u:\w)+)\s+isa\s+((?-u:\w)+)").expect("put entity regex is valid")
    })
}

fn type_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"isa\s+((?-u:\w)+)").expect("type regex is valid"))
}

fn key_has_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r#"has\s+((?-u:\w)+)\s+\"([^\"]+)\""#).expect("key has regex is valid")
    })
}

fn has_clause_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r#"has\s+((?-u:\w)+)\s+(\"(?:[^\"\\]|\\.)*\"|\S+?)(?:\s+(@(?-u:\w)+(?:\([^)]*\))?))?(?:[,;]|\s*$)"#,
        )
        .expect("has clause regex is valid")
    })
}

fn referenced_variable_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"\$((?-u:\w)+)").expect("referenced variable regex is valid"))
}

fn group_put_statements(statements: &[String]) -> Vec<PutGroup> {
    let mut groups: Vec<PutGroup> = Vec::new();
    let mut current_group: Option<PutGroup> = None;

    for statement in statements {
        if let Some(captures) = put_entity_regex().captures(statement) {
            if let Some(group) = current_group.take() {
                groups.push(group);
            }
            current_group = Some(PutGroup {
                variable: Some(captures[1].to_string()),
                type_name: Some(captures[2].to_string()),
                statements: vec![statement.clone()],
            });
        } else if current_group.as_ref().is_some_and(|group| {
            group
                .variable
                .as_ref()
                .is_some_and(|variable| statement.contains(&format!("${variable}")))
        }) {
            current_group
                .as_mut()
                .expect("current group exists")
                .statements
                .push(statement.clone());
        } else {
            if let Some(group) = current_group.take() {
                groups.push(group);
            }
            current_group = Some(PutGroup {
                variable: None,
                type_name: None,
                statements: vec![statement.clone()],
            });
        }
    }

    if let Some(group) = current_group {
        groups.push(group);
    }
    groups
}

fn extract_group_key(group: &PutGroup) -> String {
    let first_statement = &group.statements[0];
    let type_match = type_regex().captures(first_statement);
    let has_match = key_has_regex().captures(first_statement);

    match (type_match, has_match) {
        (Some(type_match), Some(has_match)) => {
            format!("{}::{}::{}", &type_match[1], &has_match[1], &has_match[2])
        }
        _ => format!(
            "raw::{}",
            js_substring(&canonical_text(std::slice::from_ref(first_statement)), 120)
        ),
    }
}

fn js_substring(value: &str, length: usize) -> String {
    let utf16: Vec<u16> = value.encode_utf16().take(length).collect();
    String::from_utf16_lossy(&utf16)
}

fn parse_has_clauses(statement: &str) -> Vec<HasClause> {
    has_clause_regex()
        .captures_iter(statement)
        .map(|captures| HasClause {
            attribute: captures[1].to_string(),
            value: captures[2].to_string(),
            annotation: captures.get(3).map(|value| value.as_str().to_string()),
        })
        .collect()
}

fn render_entity_update(
    variable: &str,
    type_name: &str,
    key_clauses: &[HasClause],
    changed_attributes: &[AttributeChange],
) -> Vec<String> {
    let mut pipelines = Vec::new();

    for change in changed_attributes {
        let old_variable = format!("${variable}_old_{}", change.attribute);
        let mut lines = vec![
            "match".to_string(),
            format!("  ${variable} isa {type_name},"),
        ];
        let key_parts: Vec<String> = key_clauses
            .iter()
            .map(|clause| format!("    has {} {}", clause.attribute, clause.value))
            .collect();
        // Without an old value there is nothing to match or delete: matching
        // `has <attribute> $old` would find no row and skip the insert.
        if change.old_values.is_empty() {
            lines.push(format!("{};", key_parts.join(",\n")));
        } else {
            lines.push(format!("{},", key_parts.join(",\n")));
            lines.push(format!("    has {} {old_variable};", change.attribute));
            lines.push("delete".to_string());
            lines.push(format!("  has {old_variable} of ${variable};"));
        }
        if !change.new_values.is_empty() {
            lines.push("insert".to_string());
            for new_value in &change.new_values {
                lines.push(format!(
                    "  ${variable} has {} {new_value};",
                    change.attribute
                ));
            }
        }

        pipelines.push(lines.join("\n"));
    }

    pipelines
}

fn attribute_map(clauses: &[HasClause], key_attribute: &str) -> Vec<(String, Vec<String>)> {
    let mut attributes: Vec<(String, Vec<String>)> = Vec::new();
    for clause in clauses {
        if clause.attribute == key_attribute {
            continue;
        }
        if let Some((_, values)) = attributes
            .iter_mut()
            .find(|(attribute, _)| attribute == &clause.attribute)
        {
            values.push(clause.value.clone());
        } else {
            attributes.push((clause.attribute.clone(), vec![clause.value.clone()]));
        }
    }
    attributes
}

fn diff_entity_group(old_group: &PutGroup, new_group: &PutGroup) -> Option<Vec<String>> {
    let old_clauses = parse_has_clauses(&old_group.statements[0]);
    let new_clauses = parse_has_clauses(&new_group.statements[0]);
    if old_clauses.is_empty() || new_clauses.is_empty() {
        return None;
    }

    let key_attribute = &new_clauses[0].attribute;
    let key_clauses: Vec<HasClause> = new_clauses
        .iter()
        .filter(|clause| clause.attribute == *key_attribute)
        .cloned()
        .collect();
    let old_attributes = attribute_map(&old_clauses, key_attribute);
    let new_attributes = attribute_map(&new_clauses, key_attribute);
    let mut changed_attributes = Vec::new();

    for (attribute, new_values) in &new_attributes {
        let old_values = old_attributes
            .iter()
            .find(|(old_attribute, _)| old_attribute == attribute)
            .map(|(_, values)| values.clone())
            .unwrap_or_default();
        if old_values != *new_values {
            changed_attributes.push(AttributeChange {
                attribute: attribute.clone(),
                old_values,
                new_values: new_values.clone(),
            });
        }
    }
    for (attribute, old_values) in &old_attributes {
        if !new_attributes
            .iter()
            .any(|(new_attribute, _)| new_attribute == attribute)
        {
            changed_attributes.push(AttributeChange {
                attribute: attribute.clone(),
                old_values: old_values.clone(),
                new_values: Vec::new(),
            });
        }
    }

    if changed_attributes.is_empty() {
        return None;
    }

    Some(render_entity_update(
        new_group.variable.as_deref().unwrap_or(""),
        new_group.type_name.as_deref().unwrap_or(""),
        &key_clauses,
        &changed_attributes,
    ))
}

fn resolve_preambles(changed_groups: &[PutGroup], all_groups: &[PutGroup]) -> Vec<PutGroup> {
    let defined_variables: HashSet<&str> = changed_groups
        .iter()
        .filter_map(|group| group.variable.as_deref())
        .collect();
    let mut needed_variables = HashSet::new();

    for group in changed_groups {
        let text = group.statements.join("\n");
        for captures in referenced_variable_regex().captures_iter(&text) {
            let variable = &captures[1];
            if !defined_variables.contains(variable) {
                needed_variables.insert(variable.to_string());
            }
        }
    }

    all_groups
        .iter()
        .filter(|group| {
            group.variable.as_ref().is_some_and(|variable| {
                needed_variables.contains(variable)
                    && !defined_variables.contains(variable.as_str())
            })
        })
        .cloned()
        .collect()
}

fn get_file_at_tag(repo_path: &Path, tag: &str, relative_path: &str) -> String {
    let revision = format!("{tag}:{relative_path}");
    let Ok(output) = Command::new("git")
        .args(["show", &revision])
        .current_dir(repo_path)
        .output()
    else {
        return String::new();
    };
    if !output.status.success() {
        return String::new();
    }
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn non_migratable_asset_paths_from_package_json(package: &Value) -> HashSet<String> {
    let mut paths = HashSet::new();
    if let Some(manifest) = package
        .get("provenance")
        .and_then(Value::as_object)
        .and_then(|provenance| provenance.get("manifest"))
        .and_then(Value::as_str)
    {
        paths.insert(manifest.to_string());
    }
    paths
}

fn joined_path(root: &Path, relative_path: &str) -> PathBuf {
    root.join(relative_path.trim_start_matches('/'))
}

fn read_text(path: &Path) -> Result<String> {
    fs::read_to_string(path)
        .map_err(|error| invalid(format!("failed to read {}: {error}", path.display())))
}

fn read_json(path: &Path) -> Result<Value> {
    let text = read_text(path)?;
    serde_json::from_str(&text)
        .map_err(|error| invalid(format!("invalid json in {}: {error}", path.display())))
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    let mut text = serde_json::to_string_pretty(value)
        .map_err(|error| invalid(format!("failed to serialize {}: {error}", path.display())))?;
    text.push('\n');
    fs::write(path, text)
        .map_err(|error| invalid(format!("failed to write {}: {error}", path.display())))
}

fn string_array(package: &Value, property: &str) -> Result<Vec<String>> {
    match package.get(property) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(ToOwned::to_owned)
                    .ok_or_else(|| invalid(format!("package.{property} entries must be strings")))
            })
            .collect(),
        Some(_) => Err(invalid(format!("package.{property} must be an array"))),
    }
}

fn package_name(package: &Value) -> String {
    match package.get("name") {
        Some(Value::String(name)) => name.clone(),
        Some(Value::Null) => "null".to_string(),
        None => "undefined".to_string(),
        Some(Value::Bool(value)) => value.to_string(),
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| match value {
                Value::Null => String::new(),
                Value::String(value) => value.clone(),
                Value::Bool(value) => value.to_string(),
                Value::Number(value) => value.to_string(),
                Value::Array(_) | Value::Object(_) => js_complex_string(value),
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".to_string(),
    }
}

fn js_complex_string(value: &Value) -> String {
    match value {
        Value::Array(values) => values
            .iter()
            .map(|value| match value {
                Value::Null => String::new(),
                Value::String(value) => value.clone(),
                Value::Bool(value) => value.to_string(),
                Value::Number(value) => value.to_string(),
                Value::Array(_) | Value::Object(_) => js_complex_string(value),
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".to_string(),
        _ => unreachable!("only arrays and objects are complex JS strings"),
    }
}

fn js_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

fn append_manifest_artifact(
    repo_path: &Path,
    manifest_relative_path: &str,
    migration_relative_path: &str,
    migration_content: &str,
) -> Result<()> {
    let manifest_path = joined_path(repo_path, manifest_relative_path);
    let mut manifest = read_json(&manifest_path)?;
    let manifest_object = manifest.as_object_mut().ok_or_else(|| {
        invalid(format!(
            "invalid json object in {}",
            manifest_path.display()
        ))
    })?;
    let artifacts = manifest_object
        .entry("artifacts")
        .or_insert_with(|| Value::Array(Vec::new()));
    if !js_truthy(artifacts) {
        *artifacts = Value::Array(Vec::new());
    }
    let artifacts = artifacts.as_array_mut().ok_or_else(|| {
        invalid(format!(
            "manifest artifacts must be an array in {}",
            manifest_path.display()
        ))
    })?;
    artifacts.push(serde_json::json!({
        "kind": "migration",
        "path": migration_relative_path,
        "sha256": hash_bytes(migration_content.as_bytes()),
    }));
    write_json(&manifest_path, &manifest)
}

/// What [`generate_upgrade`] wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedUpgrade {
    /// The directory `migrations/v<from>-to-v<to>` holding the units.
    pub path: String,
    /// The units written, `migrations/v<from>-to-v<to>/NNNN.tql`, in apply
    /// order. Each runs as one query in its own transaction.
    pub files: Vec<String>,
}

fn schema_resource_write_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?m)^\s*(?:put|insert)\s+\$[A-Za-z][A-Za-z0-9_]*\s+isa\s+SchemaResource\b")
            .expect("schema resource regex is valid")
    })
}

/// Mirrors the app's `OntologyPackage.provenanceWriteQueryAssetPaths` for data
/// files: a `.tql` path with a component naming provenance.
pub fn is_provenance_path(relative_path: &str) -> bool {
    relative_path.ends_with(".tql")
        && relative_path
            .split('/')
            .any(|component| component.to_lowercase().contains("provenance"))
}

/// Mirrors the app's `PackageBootstrapUnit.isSchemaDocumentation`.
fn is_schema_documentation(relative_path: &str, text: &str) -> bool {
    if is_provenance_path(relative_path) {
        return false;
    }
    (relative_path.contains("schema-docs") && relative_path.ends_with(".tql"))
        || schema_resource_write_regex().is_match(text)
}

/// Files an upgrade never carries. After every upgrade the app deletes the
/// package's schema documentation and re-applies it, and re-applies the
/// provenance files (`OntologyViewModel+TypeDB.swift`, `finalWriteUnits`), so
/// both are rebuilt from the release itself. Provenance records
/// (`SourceArtifactRecord`, …) also have no key, so a change to one cannot be
/// matched to a single record.
fn is_rebuilt_by_the_app(relative_path: &str, text: &str) -> bool {
    is_provenance_path(relative_path) || is_schema_documentation(relative_path, text)
}

/// The group's text with its variables renamed in order of appearance, so two
/// groups that differ only in generated variable names compare equal.
fn canonical_text(statements: &[String]) -> String {
    let text = statements.join("\n");
    let mut names: HashMap<String, usize> = HashMap::new();
    referenced_variable_regex()
        .replace_all(&text, |captures: &regex::Captures<'_>| {
            let next = names.len();
            let id = *names.entry(captures[1].to_string()).or_insert(next);
            format!("$v{id}")
        })
        .into_owned()
}

fn has_stage(group: &PutGroup, keyword: &str) -> bool {
    pipeline_stages(&group.statements.join("\n"))
        .iter()
        .any(|stage| stage.keyword == keyword)
}

fn group_text(groups: &[PutGroup]) -> String {
    groups
        .iter()
        .map(|group| group.statements.join("\n"))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn data_files_of(package: &Value) -> Result<Vec<String>> {
    let non_migratable_paths = non_migratable_asset_paths_from_package_json(package);
    Ok(string_array(package, "data")?
        .into_iter()
        .filter(|path| !non_migratable_paths.contains(path))
        .collect())
}

fn remove_existing(path: &Path) -> Result<()> {
    let result = if path.is_dir() {
        fs::remove_dir_all(path)
    } else if path.exists() {
        fs::remove_file(path)
    } else {
        return Ok(());
    };
    result.map_err(|error| invalid(format!("failed to remove {}: {error}", path.display())))
}

fn write_file(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            invalid(format!(
                "failed to create directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    fs::write(path, content)
        .map_err(|error| invalid(format!("failed to write {}: {error}", path.display())))
}

/// Writes the schema unit of a generated plan,
/// `migrations/v<from>-to-v<to>-schema.tql`: the package's schema files in
/// load order, without their `fun` definitions. `define` of a type that is
/// already installed changes nothing, so applied over the previous release it
/// adds exactly what the new release adds, before the plan's write units run.
/// Functions are left to the app's package schema apply that follows the plan,
/// which redefines installed functions instead of re-defining them (TypeDB
/// refuses that with FUN5). Returns the relative path.
pub fn write_schema_upgrade(
    repo_path: &Path,
    from_version: &str,
    to_version: &str,
) -> Result<String> {
    let package = read_json(&repo_path.join("package.json"))?;
    let schema_files: Vec<String> = package
        .get("schemas")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|schema| schema.get("file").and_then(Value::as_str))
        .map(ToOwned::to_owned)
        .collect();
    let load_order: Vec<String> = package
        .pointer("/assembly/loadOrder")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|path| schema_files.iter().any(|file| file == path))
        .map(ToOwned::to_owned)
        .collect();
    let ordered = if load_order.is_empty() {
        schema_files
    } else {
        load_order
    };

    let mut parts = vec![format!(
        "# Schema unit: {} v{from_version} → v{to_version}\n\
         # Generated by ontology-release from the package schema files, without functions.\n\
         # Applied before the upgrade's write units; defining an installed type changes nothing.",
        package_name(&package)
    )];
    for file in &ordered {
        let (types, _) = split_functions(&read_text(&joined_path(repo_path, file))?);
        let significant = types
            .lines()
            .map(|line| line.split('#').next().unwrap_or("").trim())
            .any(|line| !line.is_empty() && line != "define");
        if significant {
            parts.push(format!("# from {file}\n{}", types.trim()));
        }
    }
    let relative_path = format!("migrations/v{from_version}-to-v{to_version}-schema.tql");
    write_file(
        &joined_path(repo_path, &relative_path),
        &format!("{}\n", parts.join("\n\n")),
    )?;
    Ok(relative_path)
}

/// Generates the upgrade from `v{from_version}` to the current working tree,
/// returning the directory of units or `None` when no upgrade data changed.
/// See [`generate_upgrade`].
pub fn generate_migration_diff(
    repo_path: &Path,
    from_version: &str,
    to_version: &str,
) -> Result<Option<String>> {
    Ok(generate_upgrade(repo_path, from_version, to_version)?.map(|upgrade| upgrade.path))
}

/// Generates the data upgrade from `v{from_version}` (the git tag) to the
/// current working tree, as ordered units
/// `migrations/v<from>-to-v<to>/NNNN.tql`, or `None` when no upgrade data
/// changed. A single file `migrations/v<from>-to-v<to>.tql` is never written:
/// the app would send it as one query, chaining every change in one pipeline.
///
/// - Only rows that changed are written. Rows are matched by their key across
///   all of a version's data files, ignoring generated variable names.
/// - Schema documentation and provenance are left out: the app rebuilds both
///   after every upgrade.
/// - New or rewritten put/update groups (no `match`) share units, within the
///   data write-unit limits, each unit carrying the puts its groups refer to.
/// - Every `match` change is a unit of its own. A changed attribute becomes
///   `match` (by the row's `@key`) / `delete` / `insert`; a changed row whose
///   first attribute is not a key of its type is refused, since its match
///   would select more than that row.
pub fn generate_upgrade(
    repo_path: &Path,
    from_version: &str,
    to_version: &str,
) -> Result<Option<GeneratedUpgrade>> {
    let package_path = repo_path.join("package.json");
    let package = read_json(&package_path)?;
    let data_files = data_files_of(&package)?;
    let from_tag = format!("v{from_version}");
    let base_relative_path = format!("migrations/v{from_version}-to-v{to_version}");
    remove_existing(&joined_path(
        repo_path,
        &format!("{base_relative_path}.tql"),
    ))?;
    remove_existing(&joined_path(repo_path, &base_relative_path))?;
    if data_files.is_empty() {
        return Ok(None);
    }
    let keys = SchemaKeys::for_package(repo_path, &package);

    // The previous release's rows, keyed across all of its data files.
    let old_data_files =
        serde_json::from_str::<Value>(&get_file_at_tag(repo_path, &from_tag, "package.json"))
            .ok()
            .and_then(|old_package| data_files_of(&old_package).ok())
            .unwrap_or_else(|| data_files.clone());
    let mut old_map: HashMap<String, PutGroup> = HashMap::new();
    for data_file in &old_data_files {
        let old_text = get_file_at_tag(repo_path, &from_tag, data_file);
        if is_rebuilt_by_the_app(data_file, &old_text) {
            continue;
        }
        for group in group_put_statements(&split_put_statements(&old_text)) {
            old_map.insert(extract_group_key(&group), group);
        }
    }

    let name = package_name(&package);
    let manifest_relative_path = string_array(&package, "manifests")?
        .into_iter()
        .find(|path| path.ends_with(".package-manifest.json"));
    let manifest_line = manifest_relative_path
        .as_ref()
        .map(|path| format!("# manifest: {path}\n"))
        .unwrap_or_default();
    let header = |index: usize, count: usize| {
        format!(
            "# Upgrade unit {index} of {count}: {name} v{from_version} → v{to_version}\n\
             # Generated by ontology-release\n\
             # Apply each unit in order, as one write query in its own transaction, after the \
             package schemas\n\
             {manifest_line}\n"
        )
    };
    let header_chars = header(9999, 9999).encode_utf16().count() + 2;

    let mut units: Vec<String> = Vec::new();
    for data_file in &data_files {
        let new_text = read_text(&joined_path(repo_path, data_file))?;
        if is_rebuilt_by_the_app(data_file, &new_text) {
            continue;
        }
        let current_groups = group_put_statements(&split_put_statements(&new_text));
        let mut put_groups = Vec::new();
        let mut match_groups = Vec::new();
        let mut attribute_changes = Vec::new();
        for group in &current_groups {
            let rewritten = match old_map.get(&extract_group_key(group)) {
                None => true,
                Some(old_group)
                    if canonical_text(&old_group.statements)
                        != canonical_text(&group.statements) =>
                {
                    // A group that sets its attributes with `update` is safe
                    // to run again as it is.
                    if has_stage(group, "update") || has_stage(group, "match") {
                        true
                    } else if let Some(changes) = diff_entity_group(old_group, group) {
                        let type_name = group.type_name.as_deref().unwrap_or("");
                        let key = parse_has_clauses(&group.statements[0])
                            .into_iter()
                            .next()
                            .expect("a diffed group has attributes");
                        if keys.is_key(type_name, &key.attribute) != Some(true) {
                            return Err(invalid(format!(
                                "cannot write an upgrade for the changed {type_name} with {} {} \
                                 in {data_file}: {} is not a @key or @unique attribute of \
                                 {type_name} in the package schemas, so a match on it would \
                                 select every {type_name} with that value, not this one. Give \
                                 {type_name} a key, or add the row as a new one.",
                                key.attribute, key.value, key.attribute
                            )));
                        }
                        attribute_changes.extend(changes);
                        false
                    } else {
                        true
                    }
                }
                Some(_) => false,
            };
            if rewritten {
                if has_stage(group, "match") {
                    match_groups.push(group.clone());
                } else {
                    put_groups.push(group.clone());
                }
            }
        }

        if !put_groups.is_empty() {
            let mut with_preambles = resolve_preambles(&put_groups, &current_groups);
            with_preambles.extend(put_groups.iter().cloned());
            for chunk in split_write_text(&group_text(&with_preambles), header_chars)? {
                units.push(chunk.join("\n\n"));
            }
        }
        for group in &match_groups {
            let mut with_preambles =
                resolve_preambles(std::slice::from_ref(group), &current_groups);
            with_preambles.push(group.clone());
            units.push(group_text(&with_preambles));
        }
        units.extend(attribute_changes);
    }

    if units.is_empty() {
        return Ok(None);
    }

    let count = units.len();
    let mut written = Vec::new();
    for (index, unit) in units.iter().enumerate() {
        let relative_path = format!("{base_relative_path}/{:04}.tql", index + 1);
        let content = format!("{}{unit}\n", header(index + 1, count));
        check_upgrade_unit(&relative_path, &content, &keys)?;
        written.push((relative_path, content));
    }
    for (relative_path, content) in &written {
        write_file(&joined_path(repo_path, relative_path), content)?;
        if let Some(manifest_relative_path) = &manifest_relative_path {
            append_manifest_artifact(repo_path, manifest_relative_path, relative_path, content)?;
        }
    }

    Ok(Some(GeneratedUpgrade {
        path: base_relative_path,
        files: written.into_iter().map(|(path, _)| path).collect(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn group(variable: Option<&str>, type_name: Option<&str>, statements: &[&str]) -> PutGroup {
        PutGroup {
            variable: variable.map(ToOwned::to_owned),
            type_name: type_name.map(ToOwned::to_owned),
            statements: statements
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
        }
    }

    fn git(repo_path: &Path, arguments: &[&str]) {
        let status = Command::new("git")
            .args(arguments)
            .current_dir(repo_path)
            .status()
            .unwrap();
        assert!(status.success(), "git {arguments:?} failed");
    }

    fn initialize_fixture_repo(repo_path: &Path) {
        git(repo_path, &["init", "-b", "main"]);
        git(repo_path, &["config", "user.name", "Fixture"]);
        git(repo_path, &["config", "user.email", "fixture@example.com"]);
        git(repo_path, &["add", "."]);
        git(repo_path, &["commit", "-m", "Initial fixture"]);
        git(repo_path, &["tag", "v1.0.0"]);
    }

    #[test]
    fn non_migratable_paths_exclude_manifest_but_keep_provenance_data_migratable() {
        assert!(non_migratable_asset_paths_from_package_json(&json!({
            "provenance": ["data/build.tql"]
        }))
        .is_empty());
        assert_eq!(
            non_migratable_asset_paths_from_package_json(&json!({
                "provenance": {
                    "files": ["data/build.tql"],
                    "manifest": "manifests/build.package-manifest.json"
                }
            })),
            HashSet::from(["manifests/build.package-manifest.json".to_string()])
        );
        assert!(non_migratable_asset_paths_from_package_json(&json!({
            "assembly": {
                "generatedArtifacts": [
                    "data/example-provenance.tql",
                    "data/example-schema-docs.tql"
                ]
            }
        }))
        .is_empty());
    }

    #[test]
    fn split_put_statements_parses_multi_line_put_statements() {
        let statements = split_put_statements(
            r#"
# comment
put $r1 isa SchemaResource,
  has docKey "key1",
  has typeLabel "Type1";
put (resource: $r1, module: $module) isa inModule;

put $r2 isa SchemaResource,
  has docKey "key2";
"#,
        );
        assert_eq!(statements.len(), 3);
        assert!(statements[0].starts_with("put $r1 isa SchemaResource,"));
        assert!(statements[0].contains("has docKey \"key1\""));
        assert!(statements[1].starts_with("put (resource: $r1"));
        assert!(statements[2].starts_with("put $r2 isa SchemaResource,"));
    }

    #[test]
    fn split_put_statements_handles_empty_input() {
        assert!(split_put_statements("").is_empty());
        assert!(split_put_statements("# just comments\n# more").is_empty());
    }

    #[test]
    fn group_put_statements_groups_entity_with_its_relation() {
        let statements = vec![
            "put $module isa SchemaModule,\n  has moduleKey \"vibemachine\";".to_string(),
            "put $r1 isa SchemaResource,\n  has docKey \"key1\";".to_string(),
            "put (resource: $r1, module: $module) isa inModule;".to_string(),
            "put $r2 isa SchemaResource,\n  has docKey \"key2\";".to_string(),
            "put (resource: $r2, module: $module) isa inModule;".to_string(),
        ];
        let groups = group_put_statements(&statements);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].variable.as_deref(), Some("module"));
        assert_eq!(groups[0].statements.len(), 1);
        assert_eq!(groups[1].variable.as_deref(), Some("r1"));
        assert_eq!(groups[1].statements.len(), 2);
        assert_eq!(groups[2].variable.as_deref(), Some("r2"));
        assert_eq!(groups[2].statements.len(), 2);
    }

    #[test]
    fn extract_group_key_uses_type_and_first_has_attribute() {
        let value = group(
            Some("r1"),
            Some("SchemaResource"),
            &["put $r1 isa SchemaResource,\n  has docKey \"https://example.com#Foo\",\n  has typeLabel \"Foo\";"],
        );
        assert_eq!(
            extract_group_key(&value),
            "SchemaResource::docKey::https://example.com#Foo"
        );
    }

    #[test]
    fn extract_group_key_falls_back_to_raw_prefix_for_keyless_statements() {
        let value = group(
            None,
            None,
            &["put (resource: $r1, module: $module) isa inModule;"],
        );
        assert!(extract_group_key(&value).starts_with("raw::"));
    }

    #[test]
    fn resolve_preambles_includes_referenced_but_undefined_variables() {
        let module_group = group(
            Some("module"),
            Some("SchemaModule"),
            &["put $module isa SchemaModule, has moduleKey \"test\";"],
        );
        let changed_group = group(
            Some("r1"),
            Some("SchemaResource"),
            &[
                "put $r1 isa SchemaResource, has docKey \"key1\";",
                "put (resource: $r1, module: $module) isa inModule;",
            ],
        );
        let preambles = resolve_preambles(
            std::slice::from_ref(&changed_group),
            &[module_group, changed_group.clone()],
        );
        assert_eq!(preambles.len(), 1);
        assert_eq!(preambles[0].variable.as_deref(), Some("module"));
    }

    #[test]
    fn resolve_preambles_returns_empty_when_all_variables_are_self_contained() {
        let value = group(
            Some("draft"),
            Some("SpecificationStatus"),
            &["put $draft isa SpecificationStatus, has status_label \"draft\";"],
        );
        assert!(
            resolve_preambles(std::slice::from_ref(&value), std::slice::from_ref(&value))
                .is_empty()
        );
    }

    #[test]
    fn parse_has_clauses_extracts_attribute_value_pairs() {
        let clauses = parse_has_clauses(
            "put $r1 isa SchemaResource,\n  has docKey \"key1\",\n  has typeLabel \"Type1\",\n  has scopeNote \"some note\";",
        );
        assert_eq!(clauses.len(), 3);
        assert_eq!(clauses[0].attribute, "docKey");
        assert_eq!(clauses[0].value, "\"key1\"");
        assert_eq!(clauses[1].attribute, "typeLabel");
        assert_eq!(clauses[2].attribute, "scopeNote");
        assert_eq!(clauses[2].value, "\"some note\"");
    }

    #[test]
    fn diff_entity_group_generates_update_for_changed_scope_note() {
        let old_group = group(
            Some("r1"),
            Some("SchemaResource"),
            &[
                "put $r1 isa SchemaResource,\n  has docKey \"key1\",\n  has typeLabel \"Type1\",\n  has scopeNote \"old note\";",
                "put (resource: $r1, module: $module) isa inModule;",
            ],
        );
        let new_group = group(
            Some("r1"),
            Some("SchemaResource"),
            &[
                "put $r1 isa SchemaResource,\n  has docKey \"key1\",\n  has typeLabel \"Type1\",\n  has scopeNote \"new note\";",
                "put (resource: $r1, module: $module) isa inModule;",
            ],
        );
        let result = diff_entity_group(&old_group, &new_group)
            .unwrap()
            .join("\n\n");
        assert!(result.contains("match"));
        assert!(result.contains("has docKey \"key1\""));
        assert!(result.contains("has scopeNote $r1_old_scopeNote"));
        assert!(result.contains("delete"));
        assert!(result.contains("has $r1_old_scopeNote of $r1"));
        assert!(result.contains("insert"));
        assert!(result.contains("\"new note\""));
        assert!(!result.contains("typeLabel"));
    }

    #[test]
    fn diff_entity_group_returns_none_when_only_relation_puts_changed() {
        let old_group = group(
            Some("r1"),
            Some("SchemaResource"),
            &[
                "put $r1 isa SchemaResource,\n  has docKey \"key1\",\n  has scopeNote \"same\";",
                "put (resource: $r1, module: $old_module) isa inModule;",
            ],
        );
        let new_group = group(
            Some("r1"),
            Some("SchemaResource"),
            &[
                "put $r1 isa SchemaResource,\n  has docKey \"key1\",\n  has scopeNote \"same\";",
                "put (resource: $r1, module: $new_module) isa inModule;",
            ],
        );
        assert_eq!(diff_entity_group(&old_group, &new_group), None);
    }

    #[test]
    fn full_diff_flow_updates_changed_entity_and_puts_new_entity() {
        let old_text = r#"
put $module isa SchemaModule,
  has moduleKey "test",
  has moduleName "test";

put $r1 isa SchemaResource,
  has docKey "key1",
  has scopeNote "old note";
put (resource: $r1, module: $module) isa inModule;
"#;
        let new_text = r#"
put $module isa SchemaModule,
  has moduleKey "test",
  has moduleName "test";

put $r1 isa SchemaResource,
  has docKey "key1",
  has scopeNote "new note";
put (resource: $r1, module: $module) isa inModule;

put $r3 isa SchemaResource,
  has docKey "key3",
  has scopeNote "brand new";
put (resource: $r3, module: $module) isa inModule;
"#;
        let old_groups = group_put_statements(&split_put_statements(old_text));
        let new_groups = group_put_statements(&split_put_statements(new_text));
        let old_map: HashMap<String, PutGroup> = old_groups
            .into_iter()
            .map(|group| (extract_group_key(&group), group))
            .collect();
        let mut new_puts = Vec::new();
        let mut updates = Vec::new();
        for group in new_groups {
            let key = extract_group_key(&group);
            match old_map.get(&key) {
                None => new_puts.push(group),
                Some(old_group)
                    if old_group.statements.join("\n") != group.statements.join("\n") =>
                {
                    if let Some(update) = diff_entity_group(old_group, &group) {
                        updates.push(update.join("\n\n"));
                    }
                }
                Some(_) => {}
            }
        }
        assert_eq!(updates.len(), 1);
        assert!(updates[0].contains("has docKey \"key1\""));
        assert!(updates[0].contains("\"new note\""));
        assert_eq!(new_puts.len(), 1);
        assert_eq!(new_puts[0].variable.as_deref(), Some("r3"));
    }

    const FIXTURE_SCHEMA: &str = "define\n  entity Seed, owns seedKey @key, owns label @card(0..), owns note;\n  entity Record, owns path, owns sha;\n  attribute seedKey, value string;\n";

    fn release_fixture(old_files: &[(&str, String)], new_files: &[(&str, String)]) -> TempDir {
        let directory = TempDir::new().unwrap();
        let repo_path = directory.path().join("repo");
        let write_package = |files: &[(&str, String)]| {
            for (path, text) in files {
                let absolute = repo_path.join(path);
                fs::create_dir_all(absolute.parent().unwrap()).unwrap();
                fs::write(absolute, text).unwrap();
            }
            let data: Vec<&str> = files.iter().map(|(path, _)| *path).collect();
            write_json(
                &repo_path.join("package.json"),
                &json!({
                    "name": "fixture",
                    "version": "1.0.0",
                    "schemas": [{"name": "fixture", "file": "schema/fixture.tql"}],
                    "data": data,
                    "manifests": ["manifests/fixture.package-manifest.json"]
                }),
            )
            .unwrap();
        };
        fs::create_dir_all(repo_path.join("schema")).unwrap();
        fs::write(repo_path.join("schema/fixture.tql"), FIXTURE_SCHEMA).unwrap();
        fs::create_dir_all(repo_path.join("manifests")).unwrap();
        write_json(
            &repo_path.join("manifests/fixture.package-manifest.json"),
            &json!({"name": "fixture", "artifacts": []}),
        )
        .unwrap();
        write_package(old_files);
        initialize_fixture_repo(&repo_path);
        for (path, _) in old_files {
            fs::remove_file(repo_path.join(path)).unwrap();
        }
        write_package(new_files);
        directory
    }

    fn unit_texts(repo_path: &Path, upgrade: &GeneratedUpgrade) -> Vec<String> {
        upgrade
            .files
            .iter()
            .map(|file| read_text(&repo_path.join(file)).unwrap())
            .collect()
    }

    fn docs(resources: &[&str]) -> String {
        let mut text = "put $module isa SchemaModule, has moduleKey \"fixture\";\n\n".to_string();
        for (index, resource) in resources.iter().enumerate() {
            text.push_str(&format!(
                "put $resource_{index} isa SchemaResource, has docKey \"{resource}\";\nput (resource: $resource_{index}, module: $module) isa inModule;\n\n"
            ));
        }
        text
    }

    #[test]
    fn upgrade_is_units_only_with_exact_text_and_manifest_hash() {
        let directory = release_fixture(
            &[(
                "data/seed.tql",
                "put $seed isa Seed,\n  has seedKey \"seed-1\";\n".to_string(),
            )],
            &[(
                "data/seed.tql",
                "put $seed isa Seed,\n  has seedKey \"seed-2\";\n".to_string(),
            )],
        );
        let repo_path = directory.path().join("repo");
        let upgrade = generate_upgrade(&repo_path, "1.0.0", "1.0.1")
            .unwrap()
            .unwrap();
        assert_eq!(upgrade.path, "migrations/v1.0.0-to-v1.0.1");
        assert_eq!(upgrade.files, ["migrations/v1.0.0-to-v1.0.1/0001.tql"]);
        assert!(!repo_path.join("migrations/v1.0.0-to-v1.0.1.tql").exists());
        let expected = concat!(
            "# Upgrade unit 1 of 1: fixture v1.0.0 → v1.0.1\n",
            "# Generated by ontology-release\n",
            "# Apply each unit in order, as one write query in its own transaction, after the package schemas\n",
            "# manifest: manifests/fixture.package-manifest.json\n",
            "\n",
            "put $seed isa Seed,\n",
            "  has seedKey \"seed-2\";\n"
        );
        assert_eq!(unit_texts(&repo_path, &upgrade), [expected]);
        let manifest =
            read_json(&repo_path.join("manifests/fixture.package-manifest.json")).unwrap();
        let artifact = manifest["artifacts"].as_array().unwrap().last().unwrap();
        assert_eq!(artifact["kind"], "migration");
        assert_eq!(artifact["path"], upgrade.files[0]);
        assert_eq!(artifact["sha256"], hash_bytes(expected.as_bytes()));
    }

    #[test]
    fn upgrade_leaves_out_provenance_and_schema_documentation_the_app_rebuilds() {
        let provenance = |version: &str, sha: &str| {
            format!(
                "put $version isa OntologyModuleVersion,\n  has moduleVersionKey \"fixture@{version}\";\n\n\
                 put $s1 isa Record,\n  has path \"package.json\",\n  has sha \"{sha}\";\n"
            )
        };
        let directory = release_fixture(
            &[
                (
                    "generated/apply-units/data/fixture-schema-docs/0001.tql",
                    docs(&["a", "b"]),
                ),
                ("data/fixture-provenance.tql", provenance("1.0.0", "old")),
                (
                    "data/seed.tql",
                    "put $s isa Seed, has seedKey \"one\";\n".to_string(),
                ),
            ],
            &[
                (
                    "generated/apply-units/data/fixture-schema-docs/0001.tql",
                    docs(&["a", "new"]),
                ),
                (
                    "generated/apply-units/data/fixture-schema-docs/0002.tql",
                    docs(&["b"]),
                ),
                ("data/fixture-provenance.tql", provenance("1.0.1", "new")),
                (
                    "data/seed.tql",
                    "put $s isa Seed, has seedKey \"one\";\n".to_string(),
                ),
            ],
        );
        let repo_path = directory.path().join("repo");
        assert_eq!(
            generate_upgrade(&repo_path, "1.0.0", "1.0.1").unwrap(),
            None
        );
    }

    #[test]
    fn rows_that_moved_file_or_were_renumbered_are_unchanged() {
        let directory = release_fixture(
            &[(
                "data/a.tql",
                "put $c1 isa Seed, has seedKey \"x\";\nput $c2 isa Seed, has seedKey \"y\";\n"
                    .to_string(),
            )],
            &[
                (
                    "data/a.tql",
                    "put $c7 isa Seed, has seedKey \"y\";\n".to_string(),
                ),
                (
                    "data/b.tql",
                    "put $c1 isa Seed, has seedKey \"x\";\nput $c2 isa Seed, has seedKey \"z\";\n"
                        .to_string(),
                ),
            ],
        );
        let repo_path = directory.path().join("repo");
        let upgrade = generate_upgrade(&repo_path, "1.0.0", "1.0.1")
            .unwrap()
            .unwrap();
        let text = unit_texts(&repo_path, &upgrade).join("\n");
        assert!(text.contains("has seedKey \"z\""), "{text}");
        assert!(!text.contains("has seedKey \"x\""), "{text}");
        assert!(!text.contains("has seedKey \"y\""), "{text}");
    }

    #[test]
    fn each_match_change_is_a_unit_of_its_own_selected_by_the_key() {
        let directory = release_fixture(
            &[(
                "data/seed.tql",
                "put $a isa Seed, has seedKey \"a\", has note \"old a\";\n\n\
                 put $b isa Seed, has seedKey \"b\";\n"
                    .to_string(),
            )],
            &[(
                "data/seed.tql",
                "put $a isa Seed, has seedKey \"a\", has note \"new a\";\n\n\
                 put $b isa Seed, has seedKey \"b\", has note \"first b\";\n\n\
                 put $c isa Seed, has seedKey \"c\";\n\n\
                 put $d isa Seed, has seedKey \"d\";\n"
                    .to_string(),
            )],
        );
        let repo_path = directory.path().join("repo");
        let upgrade = generate_upgrade(&repo_path, "1.0.0", "1.0.1")
            .unwrap()
            .unwrap();
        let bodies: Vec<String> = unit_texts(&repo_path, &upgrade)
            .into_iter()
            .map(|text| text.split("\n\n").skip(1).collect::<Vec<_>>().join("\n\n"))
            .collect();
        assert_eq!(
            bodies,
            [
                "put $c isa Seed, has seedKey \"c\";\n\nput $d isa Seed, has seedKey \"d\";\n",
                "match\n  $a isa Seed,\n    has seedKey \"a\",\n    has note $a_old_note;\ndelete\n  has $a_old_note of $a;\ninsert\n  $a has note \"new a\";\n",
                "match\n  $b isa Seed,\n    has seedKey \"b\";\ninsert\n  $b has note \"first b\";\n",
            ]
        );
    }

    #[test]
    fn a_changed_row_without_a_key_is_refused_with_an_exact_message() {
        let directory = release_fixture(
            &[(
                "data/records.tql",
                "put $r isa Record, has path \"package.json\", has sha \"1\";\n".to_string(),
            )],
            &[(
                "data/records.tql",
                "put $r isa Record, has path \"package.json\", has sha \"2\";\n".to_string(),
            )],
        );
        let repo_path = directory.path().join("repo");
        let error = generate_upgrade(&repo_path, "1.0.0", "1.0.1")
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(
                "cannot write an upgrade for the changed Record with path \"package.json\" in \
                 data/records.tql: path is not a @key or @unique attribute of Record in the \
                 package schemas, so a match on it would select every Record with that value, \
                 not this one. Give Record a key, or add the row as a new one."
            ),
            "{error}"
        );
    }

    #[test]
    fn files_that_reuse_a_variable_for_different_types_never_share_a_unit() {
        let directory = release_fixture(
            &[("data/a.tql", String::new()), ("data/b.tql", String::new())],
            &[
                (
                    "data/a.tql",
                    "put $module isa Seed, has seedKey \"a\";\n".to_string(),
                ),
                (
                    "data/b.tql",
                    "put $module isa Record, has path \"b\";\n".to_string(),
                ),
            ],
        );
        let repo_path = directory.path().join("repo");
        let upgrade = generate_upgrade(&repo_path, "1.0.0", "1.0.1")
            .unwrap()
            .unwrap();
        let texts = unit_texts(&repo_path, &upgrade);
        assert_eq!(texts.len(), 2);
        assert!(texts[0].contains("isa Seed") && !texts[0].contains("isa Record"));
        assert!(texts[1].contains("isa Record") && !texts[1].contains("isa Seed"));
    }

    #[test]
    fn a_large_upgrade_is_split_into_units_within_limits() {
        let seeds: String = (0..1100)
            .map(|index| format!("put $s{index} isa Seed, has seedKey \"seed-{index}\";\n"))
            .collect();
        let directory = release_fixture(
            &[("data/seed.tql", String::new())],
            &[("data/seed.tql", seeds)],
        );
        let repo_path = directory.path().join("repo");
        let upgrade = generate_upgrade(&repo_path, "1.0.0", "1.0.1")
            .unwrap()
            .unwrap();
        assert_eq!(upgrade.files.len(), 22);
        let keys = SchemaKeys::from_schema_texts(&[FIXTURE_SCHEMA]);
        let mut seen = 0;
        for (file, text) in upgrade.files.iter().zip(unit_texts(&repo_path, &upgrade)) {
            check_upgrade_unit(file, &text, &keys).unwrap();
            seen += text.matches("isa Seed").count();
        }
        assert_eq!(seen, 1100);
        crate::upgrade_limits::validate_upgrade_files(&repo_path).unwrap();
    }
}
