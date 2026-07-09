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
use crate::executable_package::{hash_bytes, split_put_statements_on_lf as split_put_statements};

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
        _ => format!("raw::{}", js_substring(first_statement, 120)),
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
) -> String {
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
        lines.push(format!("{},", key_parts.join(",\n")));
        lines.push(format!("    has {} {old_variable};", change.attribute));

        if !change.old_values.is_empty() {
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

    pipelines.join("\n\n")
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

fn diff_entity_group(old_group: &PutGroup, new_group: &PutGroup) -> Option<String> {
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

/// Generates a migration diff between `v{from_version}` and the current
/// working tree, returning the relative migration path or `None` when no data
/// changed. Mirrors JavaScript `generateMigrationDiff` with synchronous I/O.
pub fn generate_migration_diff(
    repo_path: &Path,
    from_version: &str,
    to_version: &str,
) -> Result<Option<String>> {
    let package_path = repo_path.join("package.json");
    let package = read_json(&package_path)?;
    let non_migratable_paths = non_migratable_asset_paths_from_package_json(&package);
    let data_files: Vec<String> = string_array(&package, "data")?
        .into_iter()
        .filter(|path| !non_migratable_paths.contains(path))
        .collect();
    if data_files.is_empty() {
        return Ok(None);
    }

    let from_tag = format!("v{from_version}");
    let mut all_new_groups = Vec::new();
    let mut new_groups = Vec::new();
    let mut update_statements = Vec::new();

    for data_file in data_files {
        let old_text = get_file_at_tag(repo_path, &from_tag, &data_file);
        let new_text = read_text(&joined_path(repo_path, &data_file))?;
        let old_groups = group_put_statements(&split_put_statements(&old_text));
        let current_groups = group_put_statements(&split_put_statements(&new_text));
        all_new_groups.extend(current_groups.clone());

        let mut old_map: HashMap<String, PutGroup> = HashMap::new();
        for group in old_groups {
            old_map.insert(extract_group_key(&group), group);
        }

        for group in current_groups {
            let key = extract_group_key(&group);
            match old_map.get(&key) {
                None => new_groups.push(group),
                Some(old_group)
                    if old_group.statements.join("\n") != group.statements.join("\n") =>
                {
                    if let Some(update) = diff_entity_group(old_group, &group) {
                        update_statements.push(update);
                    } else {
                        new_groups.push(group);
                    }
                }
                Some(_) => {}
            }
        }
    }

    if new_groups.is_empty() && update_statements.is_empty() {
        return Ok(None);
    }

    let mut put_groups = if new_groups.is_empty() {
        Vec::new()
    } else {
        resolve_preambles(&new_groups, &all_new_groups)
    };
    put_groups.extend(new_groups);

    let migration_relative_path = format!("migrations/v{from_version}-to-v{to_version}.tql");
    let migration_path = joined_path(repo_path, &migration_relative_path);
    if let Some(parent) = migration_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            invalid(format!(
                "failed to create directory {}: {error}",
                parent.display()
            ))
        })?;
    }

    let manifest_relative_path = string_array(&package, "manifests")?
        .into_iter()
        .find(|path| path.ends_with(".package-manifest.json"));
    let mut header = vec![
        format!(
            "# Migration: {} v{from_version} → v{to_version}",
            package_name(&package)
        ),
        "# Generated by ontology-release".to_string(),
        "# Apply in a write transaction against an existing database".to_string(),
    ];
    if let Some(path) = &manifest_relative_path {
        header.push(format!("# manifest: {path}"));
    }
    header.push(String::new());

    let mut sections = Vec::new();
    if !put_groups.is_empty() {
        sections.push(
            put_groups
                .iter()
                .map(|group| group.statements.join("\n"))
                .collect::<Vec<_>>()
                .join("\n\n"),
        );
    }
    if !update_statements.is_empty() {
        sections.push(update_statements.join("\n\n"));
    }
    let migration_content = format!("{}\n{}\n", header.join("\n"), sections.join("\n\n"));
    fs::write(&migration_path, &migration_content).map_err(|error| {
        invalid(format!(
            "failed to write {}: {error}",
            migration_path.display()
        ))
    })?;

    if let Some(manifest_relative_path) = manifest_relative_path {
        append_manifest_artifact(
            repo_path,
            &manifest_relative_path,
            &migration_relative_path,
            &migration_content,
        )?;
    }

    Ok(Some(migration_relative_path))
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
        let result = diff_entity_group(&old_group, &new_group).unwrap();
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
                        updates.push(update);
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

    #[test]
    fn generate_migration_diff_includes_target_provenance_updates() {
        let directory = TempDir::new().unwrap();
        let repo_path = directory.path().join("repo");
        fs::create_dir_all(repo_path.join("data")).unwrap();
        let package = json!({
            "name": "fixture-package",
            "version": "1.0.1",
            "data": ["data/seed.tql", "data/provenance.tql"],
            "provenance": ["data/provenance.tql"]
        });
        write_json(&repo_path.join("package.json"), &package).unwrap();
        fs::write(
            repo_path.join("data/seed.tql"),
            "put $seed isa SeedThing,\n  has seedKey \"seed-1\";\n",
        )
        .unwrap();
        fs::write(
            repo_path.join("data/provenance.tql"),
            "put $version isa OntologyModuleVersion,\n  has moduleVersionKey \"https://example.com/fixture-package@1.0.0\";\n",
        )
        .unwrap();
        initialize_fixture_repo(&repo_path);
        fs::write(
            repo_path.join("data/seed.tql"),
            "put $seed isa SeedThing,\n  has seedKey \"seed-2\";\n",
        )
        .unwrap();
        fs::write(
            repo_path.join("data/provenance.tql"),
            "put $version isa OntologyModuleVersion,\n  has moduleVersionKey \"https://example.com/fixture-package@1.0.1\";\n",
        )
        .unwrap();

        let migration_path = generate_migration_diff(&repo_path, "1.0.0", "1.0.1").unwrap();
        assert_eq!(
            migration_path.as_deref(),
            Some("migrations/v1.0.0-to-v1.0.1.tql")
        );
        let migration_text =
            read_text(&joined_path(&repo_path, migration_path.as_ref().unwrap())).unwrap();
        assert!(migration_text.contains("seed-2"));
        assert!(migration_text.contains("OntologyModuleVersion"));
        assert!(migration_text.contains("fixture-package@1.0.1"));
    }

    #[test]
    fn generate_migration_diff_writes_exact_text_and_recomputed_manifest_hash() {
        let directory = TempDir::new().unwrap();
        let repo_path = directory.path().join("repo");
        fs::create_dir_all(repo_path.join("data")).unwrap();
        fs::create_dir_all(repo_path.join("manifests")).unwrap();
        let package = json!({
            "name": "fixture-package",
            "version": "1.0.1",
            "data": ["data/seed.tql"],
            "manifests": ["manifests/fixture.package-manifest.json"]
        });
        write_json(&repo_path.join("package.json"), &package).unwrap();
        write_json(
            &repo_path.join("manifests/fixture.package-manifest.json"),
            &json!({"name": "fixture-package", "artifacts": []}),
        )
        .unwrap();
        fs::write(
            repo_path.join("data/seed.tql"),
            "put $seed isa SeedThing,\n  has seedKey \"seed-1\";\n",
        )
        .unwrap();
        initialize_fixture_repo(&repo_path);
        fs::write(
            repo_path.join("data/seed.tql"),
            "put $seed isa SeedThing,\n  has seedKey \"seed-2\";\n",
        )
        .unwrap();

        let relative_path = generate_migration_diff(&repo_path, "1.0.0", "1.0.1")
            .unwrap()
            .unwrap();
        let migration_text = read_text(&joined_path(&repo_path, &relative_path)).unwrap();
        let expected = concat!(
            "# Migration: fixture-package v1.0.0 → v1.0.1\n",
            "# Generated by ontology-release\n",
            "# Apply in a write transaction against an existing database\n",
            "# manifest: manifests/fixture.package-manifest.json\n",
            "\n",
            "put $seed isa SeedThing,\n",
            "  has seedKey \"seed-2\";\n"
        );
        assert_eq!(migration_text, expected);

        let manifest =
            read_json(&repo_path.join("manifests/fixture.package-manifest.json")).unwrap();
        let artifact = manifest["artifacts"].as_array().unwrap().last().unwrap();
        assert_eq!(artifact["kind"], "migration");
        assert_eq!(artifact["path"], relative_path);
        assert_eq!(artifact["sha256"], hash_bytes(expected.as_bytes()));
    }
}
