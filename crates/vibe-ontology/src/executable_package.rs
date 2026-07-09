//! Executable-package preparation and validation.
//!
//! Rust port of `src/lib/executable-package.mjs`, so the CLI, CI
//! (one-2xg.18), and the app share one implementation
//! (one-2xg.16 / one-2xg.46). Validation order and error messages intentionally
//! match the JavaScript implementation.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use ring::digest::{digest, SHA256};
use serde_json::{json, Value};

use crate::error::{Error, Result};

pub const APPLY_UNITS_ROOT: &str = "generated/apply-units";
pub const MAX_WRITE_UNIT_CHARS: usize = 50_000;
pub const MAX_WRITE_UNIT_BLOCKS: usize = 50;

fn invalid<S: Into<String>>(message: S) -> Error {
    Error::Version(message.into())
}

/// Overrides for generated shard limits. `None` passed to
/// [`prepare_executable_package`] uses the JavaScript defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrepareExecutablePackageOptions {
    pub max_chars: usize,
    pub max_blocks: usize,
}

impl Default for PrepareExecutablePackageOptions {
    fn default() -> Self {
        Self {
            max_chars: MAX_WRITE_UNIT_CHARS,
            max_blocks: MAX_WRITE_UNIT_BLOCKS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PutGroup {
    variable: Option<String>,
    statements: Vec<String>,
}

fn is_non_empty_string(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
}

fn is_typeql_file(value: &Value) -> bool {
    value.as_str().is_some_and(|path| path.ends_with(".tql"))
}

fn is_generated_apply_unit(value: &Value) -> bool {
    value.as_str().is_some_and(|path| {
        path == APPLY_UNITS_ROOT || path.starts_with(&format!("{APPLY_UNITS_ROOT}/"))
    })
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

fn joined_path(root: &Path, relative_path: &str) -> PathBuf {
    root.join(relative_path.trim_start_matches('/'))
}

fn significant_lines(text: &str) -> Vec<&str> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
}

fn has_put_only(text: &str) -> bool {
    let lines = significant_lines(text);
    lines.iter().any(|line| line.starts_with("put "))
        && !lines.iter().any(|line| {
            line.starts_with("match")
                || line.starts_with("insert ")
                || line.starts_with("delete ")
                || line.starts_with("update ")
        })
}

pub(crate) fn split_put_statements(text: &str) -> Vec<String> {
    split_put_statement_lines(
        text.split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line)),
    )
}

/// The migration-diff JS source splits only on LF, unlike executable-package's
/// CRLF-aware splitter. Keep the shared algorithm while preserving that detail.
pub(crate) fn split_put_statements_on_lf(text: &str) -> Vec<String> {
    split_put_statement_lines(text.split('\n'))
}

fn split_put_statement_lines<'a>(lines: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();

    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }

        if trimmed.starts_with("put ") && !current.is_empty() {
            statements.push(current.trim().to_string());
            current = line.to_string();
        } else {
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(line);
        }
    }

    if !current.trim().is_empty() {
        statements.push(current.trim().to_string());
    }
    statements
}

fn put_entity_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"^put\s+\$((?-u:\w)+)\s+isa\s+((?-u:\w)+)").expect("put entity regex is valid")
    })
}

fn group_put_statements(statements: Vec<String>) -> Vec<PutGroup> {
    let mut groups: Vec<PutGroup> = Vec::new();

    for statement in statements {
        if let Some(captures) = put_entity_regex().captures(&statement) {
            groups.push(PutGroup {
                variable: Some(captures[1].to_string()),
                statements: vec![statement],
            });
        } else if let Some(group) = groups.last_mut() {
            let references_current = group
                .variable
                .as_ref()
                .is_some_and(|variable| statement.contains(&format!("${variable}")));
            if references_current {
                group.statements.push(statement);
            } else {
                groups.push(PutGroup {
                    variable: None,
                    statements: vec![statement],
                });
            }
        } else {
            groups.push(PutGroup {
                variable: None,
                statements: vec![statement],
            });
        }
    }

    groups
}

fn referenced_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"\$((?-u:\w)+)").expect("reference regex is valid"))
}

fn declared_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"\$([A-Za-z][A-Za-z0-9_]*)\s+isa\b").expect("declaration regex is valid")
    })
}

fn referenced_variables(text: &str) -> Vec<String> {
    referenced_regex()
        .captures_iter(text)
        .map(|captures| captures[1].to_string())
        .collect()
}

fn declared_variables_for_block(block: &str) -> Vec<String> {
    declared_regex()
        .captures_iter(block)
        .map(|captures| captures[1].to_string())
        .collect()
}

fn referenced_variables_for_block(block: &str) -> Vec<String> {
    referenced_variables(block)
}

fn resolve_preamble_indexes(target_indexes: &[usize], groups: &[PutGroup]) -> Vec<usize> {
    let target_set: HashSet<usize> = target_indexes.iter().copied().collect();
    let mut resolved_variables: HashSet<String> = target_indexes
        .iter()
        .filter_map(|index| groups[*index].variable.clone())
        .collect();
    let mut group_by_variable = HashMap::new();
    for (index, group) in groups.iter().enumerate() {
        if !target_set.contains(&index) {
            if let Some(variable) = &group.variable {
                group_by_variable.insert(variable.clone(), index);
            }
        }
    }

    fn include_variable(
        variable: &str,
        groups: &[PutGroup],
        group_by_variable: &HashMap<String, usize>,
        resolved_variables: &mut HashSet<String>,
        resolved: &mut Vec<usize>,
        visiting: &mut HashSet<String>,
    ) {
        if resolved_variables.contains(variable) || visiting.contains(variable) {
            return;
        }
        let Some(index) = group_by_variable.get(variable).copied() else {
            return;
        };

        visiting.insert(variable.to_string());
        for dependency in referenced_variables(&groups[index].statements.join("\n")) {
            include_variable(
                &dependency,
                groups,
                group_by_variable,
                resolved_variables,
                resolved,
                visiting,
            );
        }
        visiting.remove(variable);

        if !resolved.contains(&index) {
            resolved.push(index);
            resolved_variables.insert(variable.to_string());
        }
    }

    let mut resolved = Vec::new();
    let mut visiting = HashSet::new();
    for index in target_indexes {
        for variable in referenced_variables(&groups[*index].statements.join("\n")) {
            include_variable(
                &variable,
                groups,
                &group_by_variable,
                &mut resolved_variables,
                &mut resolved,
                &mut visiting,
            );
        }
    }
    resolved
}

fn split_paragraph_queries(text: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        if trimmed.is_empty() {
            if !current.is_empty() {
                blocks.push(current.join("\n").trim().to_string());
                current.clear();
            }
        } else {
            current.push(line);
        }
    }

    if !current.is_empty() {
        blocks.push(current.join("\n").trim().to_string());
    }
    blocks
}

fn paragraph_separator_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"\n\s*\n+").expect("paragraph separator regex is valid"))
}

fn statements_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"(?s:.*?;)").expect("statement regex is valid"))
}

fn atomic_write_blocks(trimmed: &str) -> Vec<String> {
    let blocks: Vec<String> = paragraph_separator_regex()
        .split(trimmed)
        .map(str::trim)
        .filter(|block| !block.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    if blocks.len() > 1 {
        return blocks;
    }

    let statements: Vec<String> = statements_regex()
        .find_iter(trimmed)
        .map(|matched| matched.as_str().trim().to_string())
        .collect();
    if statements.is_empty() {
        return vec![trimmed.to_string()];
    }

    let mut statement_blocks = Vec::new();
    let mut index = 0;
    while index < statements.len() {
        let current = &statements[index];
        let referenced = referenced_variables_for_block(current);
        let mut block_statements = vec![current.clone()];
        if let Some(next) = statements.get(index + 1) {
            if (next.starts_with("put (") || next.starts_with("match"))
                && referenced
                    .iter()
                    .any(|name| next.contains(&format!("${name}")))
            {
                block_statements.push(next.clone());
                index += 1;
            }
        }
        statement_blocks.push(block_statements.join("\n"));
        index += 1;
    }
    statement_blocks
}

/// Splits TypeQL text into executable blocks, matching
/// `splitExecutableBlocks` including comment and CRLF handling.
pub fn split_executable_blocks(text: &str) -> Vec<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    if has_put_only(trimmed) {
        return group_put_statements(split_put_statements(trimmed))
            .into_iter()
            .map(|group| group.statements.join("\n\n"))
            .collect();
    }

    split_paragraph_queries(trimmed)
}

fn js_len(text: &str) -> usize {
    text.encode_utf16().count()
}

#[cfg(test)]
fn chunk_blocks(
    blocks: &[String],
    options: PrepareExecutablePackageOptions,
) -> Result<Vec<Vec<String>>> {
    let mut chunks = Vec::new();
    let mut current = Vec::new();
    let mut current_chars = 0;

    for block in blocks {
        let block_chars = js_len(block);
        if block_chars > options.max_chars {
            return Err(invalid(format!(
                "write block exceeds safe size limit ({block_chars} chars > {}) and must be split at the source",
                options.max_chars
            )));
        }

        let separator_chars = usize::from(!current.is_empty()) * 2;
        let candidate_chars = current_chars + separator_chars + block_chars;
        if !current.is_empty()
            && (candidate_chars > options.max_chars || current.len() >= options.max_blocks)
        {
            chunks.push(current);
            current = vec![block.clone()];
            current_chars = block_chars;
            continue;
        }

        current.push(block.clone());
        current_chars = candidate_chars;
    }

    if !current.is_empty() {
        chunks.push(current);
    }
    Ok(chunks)
}

fn posix_join(parts: &[&str]) -> String {
    let mut components = Vec::new();
    for component in parts.iter().flat_map(|part| part.split('/')) {
        match component {
            "" | "." => {}
            ".." if components.last().is_some_and(|last| *last != "..") => {
                components.pop();
            }
            other => components.push(other),
        }
    }
    components.join("/")
}

fn build_shard_path(source_path: &str, index: usize) -> String {
    let stem = source_path.strip_suffix(".tql").unwrap_or(source_path);
    let filename = format!("{:04}.tql", index + 1);
    posix_join(&[APPLY_UNITS_ROOT, stem, &filename])
}

fn render_shard(source_path: &str, blocks: &[String]) -> String {
    format!(
        "# Generated executable apply unit from {source_path}\n\n{}\n",
        blocks.join("\n\n")
    )
}

fn render_shard_with_headers(
    source_path: &str,
    blocks: &[String],
    manifest_path: Option<&str>,
    upstream_commit: Option<&str>,
) -> String {
    let mut headers = vec![format!(
        "# Generated executable apply unit from {source_path}"
    )];
    if manifest_path.is_some_and(|value| !value.trim().is_empty()) {
        headers.push(format!("# manifest: {}", manifest_path.unwrap_or_default()));
    }
    if upstream_commit.is_some_and(|value| !value.trim().is_empty()) {
        headers.push(format!(
            "# upstream-commit: {}",
            upstream_commit.unwrap_or_default()
        ));
    }
    format!("{}\n\n{}\n", headers.join("\n"), blocks.join("\n\n"))
}

fn render_put_candidate(indexes: &[usize], groups: &[PutGroup]) -> Vec<String> {
    resolve_preamble_indexes(indexes, groups)
        .into_iter()
        .chain(indexes.iter().copied())
        .map(|index| groups[index].statements.join("\n\n"))
        .collect()
}

fn build_put_chunks(
    text: &str,
    options: PrepareExecutablePackageOptions,
) -> Result<Vec<Vec<String>>> {
    let groups = group_put_statements(split_put_statements(text));
    if groups.is_empty() {
        return Ok(Vec::new());
    }

    let mut chunks = Vec::new();
    let mut current_indexes = Vec::new();
    for index in 0..groups.len() {
        let mut candidate_indexes = current_indexes.clone();
        candidate_indexes.push(index);
        let rendered_blocks = render_put_candidate(&candidate_indexes, &groups);
        let rendered_text = render_shard("generated", &rendered_blocks);

        if !current_indexes.is_empty()
            && (rendered_blocks.len() > options.max_blocks
                || js_len(&rendered_text) > options.max_chars)
        {
            chunks.push(render_put_candidate(&current_indexes, &groups));
            current_indexes = vec![index];
        } else {
            current_indexes = candidate_indexes;
        }
    }

    if !current_indexes.is_empty() {
        chunks.push(render_put_candidate(&current_indexes, &groups));
    }
    assert_safe_chunks(&chunks, options)?;
    Ok(chunks)
}

fn contextual_blocks(
    blocks: &[String],
    selected_indexes: &HashSet<usize>,
    declaration_indexes: &HashMap<String, usize>,
) -> Vec<String> {
    let mut context_indexes = HashSet::new();
    let mut pending = Vec::new();

    for index in selected_indexes {
        for variable in referenced_variables_for_block(&blocks[*index]) {
            if let Some(declaration_index) = declaration_indexes.get(&variable) {
                if !selected_indexes.contains(declaration_index) {
                    pending.push(*declaration_index);
                }
            }
        }
    }

    while let Some(declaration_index) = pending.pop() {
        if selected_indexes.contains(&declaration_index)
            || context_indexes.contains(&declaration_index)
        {
            continue;
        }
        context_indexes.insert(declaration_index);
        for variable in referenced_variables_for_block(&blocks[declaration_index]) {
            if let Some(dependency_index) = declaration_indexes.get(&variable) {
                if !selected_indexes.contains(dependency_index)
                    && !context_indexes.contains(dependency_index)
                {
                    pending.push(*dependency_index);
                }
            }
        }
    }

    let mut indexes: Vec<usize> = context_indexes
        .into_iter()
        .chain(selected_indexes.iter().copied())
        .collect();
    indexes.sort_unstable();
    indexes.dedup();
    indexes
        .into_iter()
        .map(|index| blocks[index].clone())
        .collect()
}

fn build_contextual_chunks(
    text: &str,
    options: PrepareExecutablePackageOptions,
) -> Result<Vec<Vec<String>>> {
    let blocks = atomic_write_blocks(text.trim());
    if blocks.is_empty() {
        return Ok(Vec::new());
    }

    let mut declaration_indexes = HashMap::new();
    for (index, block) in blocks.iter().enumerate() {
        for variable in declared_variables_for_block(block) {
            declaration_indexes.entry(variable).or_insert(index);
        }
    }

    let mut chunks = Vec::new();
    let mut current_indexes = HashSet::new();
    for index in 0..blocks.len() {
        let mut candidate_indexes = current_indexes.clone();
        candidate_indexes.insert(index);
        let candidate_blocks = contextual_blocks(&blocks, &candidate_indexes, &declaration_indexes);
        let candidate_text = render_shard("generated", &candidate_blocks);

        if !current_indexes.is_empty()
            && (candidate_blocks.len() > options.max_blocks
                || js_len(&candidate_text) > options.max_chars)
        {
            chunks.push(contextual_blocks(
                &blocks,
                &current_indexes,
                &declaration_indexes,
            ));
            current_indexes = HashSet::from([index]);
        } else {
            current_indexes = candidate_indexes;
        }
    }

    if !current_indexes.is_empty() {
        chunks.push(contextual_blocks(
            &blocks,
            &current_indexes,
            &declaration_indexes,
        ));
    }
    assert_safe_chunks(&chunks, options)?;
    Ok(chunks)
}

fn assert_safe_chunks(
    chunks: &[Vec<String>],
    options: PrepareExecutablePackageOptions,
) -> Result<()> {
    for chunk in chunks {
        if chunk.len() > options.max_blocks {
            return Err(invalid(format!(
                "write chunk exceeds safe block limit ({} blocks > {}) and must be split at the source",
                chunk.len(),
                options.max_blocks
            )));
        }
        if js_len(&render_shard("generated", chunk)) > options.max_chars {
            return Err(invalid(format!(
                "write chunk exceeds safe size limit ({} chars) and must be split at the source",
                options.max_chars
            )));
        }
    }
    Ok(())
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

pub(crate) fn hash_bytes(bytes: &[u8]) -> String {
    digest(&SHA256, bytes)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn hash_file(repo_path: &Path, relative_path: &str) -> Result<String> {
    let path = joined_path(repo_path, relative_path);
    let bytes = fs::read(&path)
        .map_err(|error| invalid(format!("failed to read {}: {error}", path.display())))?;
    Ok(hash_bytes(&bytes))
}

fn provenance_files(package: &Value) -> Vec<Value> {
    match package.get("provenance") {
        Some(Value::Array(values)) => values.clone(),
        Some(Value::Object(provenance)) => provenance
            .get("files")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn set_provenance_files(package: &mut Value, files: Vec<Value>) {
    match package.get_mut("provenance") {
        Some(Value::Array(provenance)) => *provenance = files,
        Some(Value::Object(provenance)) => {
            provenance.insert("files".to_string(), Value::Array(files));
        }
        _ => {}
    }
}

fn schema_paths(package: &Value) -> HashSet<String> {
    package
        .get("schemas")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|schema| schema.get("file").and_then(Value::as_str))
        .map(ToOwned::to_owned)
        .collect()
}

fn should_normalize_assembly_path(package: &Value, path: &Value) -> bool {
    is_typeql_file(path)
        && path
            .as_str()
            .is_some_and(|path| !schema_paths(package).contains(path))
}

fn unique(values: Vec<Value>) -> Vec<Value> {
    let mut unique = Vec::new();
    for value in values {
        if !unique.contains(&value) {
            unique.push(value);
        }
    }
    unique
}

fn active_manifest_path(package: &Value) -> Option<String> {
    if is_non_empty_string(
        package
            .get("provenance")
            .and_then(|provenance| provenance.get("manifest")),
    ) {
        return package["provenance"]["manifest"]
            .as_str()
            .map(ToOwned::to_owned);
    }
    let manifests = package
        .get("manifests")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    manifests
        .iter()
        .filter_map(Value::as_str)
        .find(|path| path.ends_with(".package-manifest.json"))
        .or_else(|| manifests.first().and_then(Value::as_str))
        .map(ToOwned::to_owned)
}

struct PackagePreparer<'a> {
    repo_path: &'a Path,
    package: Value,
    options: PrepareExecutablePackageOptions,
    generated_paths: Vec<Value>,
    generated_artifacts: Vec<Value>,
    normalized_path_cache: HashMap<String, Vec<Value>>,
    source_text_cache: HashMap<String, String>,
    manifest_path: Option<String>,
    upstream_commit: Option<String>,
}

impl<'a> PackagePreparer<'a> {
    fn new(repo_path: &'a Path, package: Value, options: PrepareExecutablePackageOptions) -> Self {
        let manifest_path = active_manifest_path(&package);
        let upstream_commit = package
            .get("upstream")
            .and_then(|upstream| upstream.get("commit"))
            .or_else(|| {
                package
                    .get("source")
                    .and_then(|source| source.get("commit"))
            })
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        Self {
            repo_path,
            package,
            options,
            generated_paths: Vec::new(),
            generated_artifacts: Vec::new(),
            normalized_path_cache: HashMap::new(),
            source_text_cache: HashMap::new(),
            manifest_path,
            upstream_commit,
        }
    }

    fn preload_sources(&mut self) -> Result<()> {
        let mut tracked = Vec::new();
        let mut seen = HashSet::new();
        let mut add = |value: &Value| {
            if let Some(path) = value.as_str() {
                if path.ends_with(".tql") && seen.insert(path.to_string()) {
                    tracked.push(path.to_string());
                }
            }
        };

        if let Some(load_order) = self
            .package
            .get("assembly")
            .and_then(|assembly| assembly.get("loadOrder"))
            .and_then(Value::as_array)
        {
            for path in load_order {
                if should_normalize_assembly_path(&self.package, path) {
                    add(path);
                }
            }
        }
        if let Some(data) = self.package.get("data").and_then(Value::as_array) {
            for path in data {
                add(path);
            }
        }
        for path in provenance_files(&self.package) {
            add(&path);
        }
        if let Some(plans) = self
            .package
            .get("migration")
            .and_then(|migration| migration.get("plans"))
            .and_then(Value::as_array)
        {
            for plan in plans {
                if let Some(phases) = plan.get("phases").and_then(Value::as_array) {
                    for phase in phases {
                        if let Some(units) = phase.get("units").and_then(Value::as_array) {
                            for unit in units {
                                if unit.get("kind").and_then(Value::as_str) == Some("write") {
                                    if let Some(path) = unit.get("path") {
                                        add(path);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        for relative_path in tracked {
            let text = read_text(&joined_path(self.repo_path, &relative_path))?;
            self.source_text_cache.insert(relative_path, text);
        }
        Ok(())
    }

    fn remove_generated_root(&self) -> Result<()> {
        match fs::remove_dir_all(self.repo_path.join(APPLY_UNITS_ROOT)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(invalid(format!(
                "failed to remove {}: {error}",
                self.repo_path.join(APPLY_UNITS_ROOT).display()
            ))),
        }
    }

    fn source_text(&self, relative_path: &str) -> Result<String> {
        if let Some(text) = self.source_text_cache.get(relative_path) {
            return Ok(text.clone());
        }
        read_text(&joined_path(self.repo_path, relative_path))
    }

    fn normalize_path(&mut self, path: &Value) -> Result<Vec<Value>> {
        if !is_typeql_file(path) {
            return Ok(vec![path.clone()]);
        }
        let relative_path = path.as_str().expect("TypeQL paths are strings");
        if let Some(cached) = self.normalized_path_cache.get(relative_path) {
            return Ok(cached.clone());
        }

        if is_generated_apply_unit(path) {
            let absolute_path = joined_path(self.repo_path, relative_path);
            fs::create_dir_all(absolute_path.parent().unwrap_or(self.repo_path)).map_err(
                |error| {
                    invalid(format!(
                        "failed to create {}: {error}",
                        absolute_path.parent().unwrap_or(self.repo_path).display()
                    ))
                },
            )?;
            let text = self.source_text(relative_path)?;
            fs::write(&absolute_path, text).map_err(|error| {
                invalid(format!(
                    "failed to write {}: {error}",
                    absolute_path.display()
                ))
            })?;
            let result = vec![path.clone()];
            self.normalized_path_cache
                .insert(relative_path.to_string(), result.clone());
            return Ok(result);
        }

        let text = self.source_text(relative_path)?;
        let trimmed = text.trim();
        if trimmed.is_empty() || split_executable_blocks(trimmed).is_empty() {
            let result = vec![path.clone()];
            self.normalized_path_cache
                .insert(relative_path.to_string(), result.clone());
            return Ok(result);
        }

        let chunks = if has_put_only(trimmed) {
            build_put_chunks(trimmed, self.options)?
        } else {
            build_contextual_chunks(trimmed, self.options)?
        };
        let one_shard = chunks.len() == 1
            && chunks[0].len() <= MAX_WRITE_UNIT_BLOCKS
            && js_len(&render_shard_with_headers(
                relative_path,
                &chunks[0],
                self.manifest_path.as_deref(),
                self.upstream_commit.as_deref(),
            )) <= MAX_WRITE_UNIT_CHARS;
        if one_shard {
            let result = vec![path.clone()];
            self.normalized_path_cache
                .insert(relative_path.to_string(), result.clone());
            return Ok(result);
        }

        let mut emitted_paths = Vec::new();
        for (index, chunk) in chunks.iter().enumerate() {
            let shard_path = build_shard_path(relative_path, index);
            let absolute_path = self.repo_path.join(&shard_path);
            fs::create_dir_all(absolute_path.parent().unwrap_or(self.repo_path)).map_err(
                |error| {
                    invalid(format!(
                        "failed to create {}: {error}",
                        absolute_path.parent().unwrap_or(self.repo_path).display()
                    ))
                },
            )?;
            let shard_text = render_shard_with_headers(
                relative_path,
                chunk,
                self.manifest_path.as_deref(),
                self.upstream_commit.as_deref(),
            );
            fs::write(&absolute_path, &shard_text).map_err(|error| {
                invalid(format!(
                    "failed to write {}: {error}",
                    absolute_path.display()
                ))
            })?;
            let path_value = Value::String(shard_path.clone());
            emitted_paths.push(path_value.clone());
            self.generated_paths.push(path_value);
            self.generated_artifacts.push(json!({
                "kind": "apply-unit",
                "path": shard_path,
                "sha256": hash_bytes(shard_text.as_bytes()),
            }));
        }

        self.normalized_path_cache
            .insert(relative_path.to_string(), emitted_paths.clone());
        Ok(emitted_paths)
    }

    fn normalize_package_paths(&mut self) -> Result<()> {
        if let Some(data) = self.package.get("data").and_then(Value::as_array).cloned() {
            let mut next = Vec::new();
            for path in data {
                next.extend(self.normalize_path(&path)?);
            }
            self.package["data"] = Value::Array(unique(next));
        }

        let provenance = provenance_files(&self.package);
        if !provenance.is_empty() {
            let mut next = Vec::new();
            for path in provenance {
                next.extend(self.normalize_path(&path)?);
            }
            set_provenance_files(&mut self.package, next);
        }

        let load_order = self
            .package
            .get("assembly")
            .and_then(|assembly| assembly.get("loadOrder"))
            .and_then(Value::as_array)
            .cloned();
        if let Some(load_order) = load_order {
            let mut next = Vec::new();
            for path in load_order {
                if should_normalize_assembly_path(&self.package, &path) {
                    next.extend(self.normalize_path(&path)?);
                } else {
                    next.push(path);
                }
            }
            self.package["assembly"]["loadOrder"] = Value::Array(unique(next));
        }

        let plans = self
            .package
            .get("migration")
            .and_then(|migration| migration.get("plans"))
            .and_then(Value::as_array)
            .cloned();
        if let Some(mut plans) = plans {
            for plan in &mut plans {
                if let Some(phases) = plan.get_mut("phases").and_then(Value::as_array_mut) {
                    for phase in phases {
                        let units = phase
                            .get("units")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        let mut next_units = Vec::new();
                        for unit in units {
                            let is_write =
                                unit.get("kind").and_then(Value::as_str) == Some("write");
                            let path = unit.get("path").cloned().unwrap_or(Value::Null);
                            if is_write && is_typeql_file(&path) {
                                for shard_path in self.normalize_path(&path)? {
                                    let mut next_unit = unit.clone();
                                    if let Some(object) = next_unit.as_object_mut() {
                                        object.insert("path".to_string(), shard_path);
                                    }
                                    next_units.push(next_unit);
                                }
                            } else {
                                next_units.push(unit);
                            }
                        }
                        phase["units"] = Value::Array(next_units);
                    }
                }
            }
            self.package["migration"]["plans"] = Value::Array(plans);
        }

        if self.package.get("assembly").is_some_and(js_truthy) {
            let existing = self.package["assembly"]
                .get("generatedArtifacts")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|path| !is_generated_apply_unit(path));
            self.package["assembly"]["generatedArtifacts"] = Value::Array(unique(
                existing.chain(self.generated_paths.clone()).collect(),
            ));
        }
        Ok(())
    }

    fn update_manifest(&self, package_path: &Path, manifest_path: &str) -> Result<()> {
        write_json(package_path, &self.package)?;
        let manifest_absolute_path = joined_path(self.repo_path, manifest_path);
        let mut manifest = read_json(&manifest_absolute_path)?;
        let existing_artifacts = manifest
            .get("artifacts")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        manifest["artifacts"] = Value::Array(
            existing_artifacts
                .into_iter()
                .filter(|artifact| !artifact.get("path").is_some_and(is_generated_apply_unit))
                .chain(self.generated_artifacts.clone())
                .collect(),
        );

        if let Some(source_artifacts) = manifest
            .get("upstream")
            .and_then(|upstream| upstream.get("sourceArtifacts"))
            .and_then(Value::as_array)
            .cloned()
        {
            let mut next = Vec::new();
            for mut artifact in source_artifacts {
                let path = artifact
                    .get("path")
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid("source artifact path must be a string"))?;
                let hash = hash_file(self.repo_path, path)?;
                artifact["sha256"] = Value::String(hash);
                next.push(artifact);
            }
            manifest["upstream"]["sourceArtifacts"] = Value::Array(next);
        }
        write_json(&manifest_absolute_path, &manifest)
    }
}

/// Rewrites oversized executable files into generated apply units and returns
/// the updated `package.json` value, matching `prepareExecutablePackage`.
pub fn prepare_executable_package(
    repo_path: &Path,
    options: Option<PrepareExecutablePackageOptions>,
) -> Result<Value> {
    let package_path = repo_path.join("package.json");
    let package = read_json(&package_path)?;
    let mut preparer = PackagePreparer::new(repo_path, package, options.unwrap_or_default());
    preparer.preload_sources()?;
    preparer.remove_generated_root()?;
    preparer.normalize_package_paths()?;

    if let Some(manifest_path) = preparer.manifest_path.as_deref() {
        if preparer
            .update_manifest(&package_path, manifest_path)
            .is_ok()
        {
            return Ok(preparer.package);
        }
    }
    write_json(&package_path, &preparer.package)?;
    Ok(preparer.package)
}

fn assert_safe_write_unit(relative_path: &str, text: &str) -> Result<()> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(invalid(format!("write unit '{relative_path}' is empty")));
    }

    let blocks = split_executable_blocks(trimmed);
    if blocks.is_empty() {
        return Err(invalid(format!(
            "write unit '{relative_path}' does not contain executable blocks"
        )));
    }
    if blocks.len() > MAX_WRITE_UNIT_BLOCKS {
        return Err(invalid(format!(
            "write unit '{relative_path}' contains {} executable blocks; max is {MAX_WRITE_UNIT_BLOCKS}",
            blocks.len()
        )));
    }

    let char_count = js_len(trimmed);
    if char_count > MAX_WRITE_UNIT_CHARS {
        return Err(invalid(format!(
            "write unit '{relative_path}' is {char_count} chars; max is {MAX_WRITE_UNIT_CHARS}. Publish sharded apply units instead of a single large query."
        )));
    }
    Ok(())
}

fn string_array(value: Option<&Value>) -> Vec<&str> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

fn maybe_check_write_unit(
    repo_path: &Path,
    relative_path: &str,
    checked_paths: &mut HashSet<String>,
) -> Result<()> {
    if !relative_path.ends_with(".tql") || !checked_paths.insert(relative_path.to_string()) {
        return Ok(());
    }
    let path = joined_path(repo_path, relative_path);
    let text = read_text(&path)?;
    assert_safe_write_unit(relative_path, &text)
}

/// Validates all executable paths in `package`, returning the first violation
/// in the same order as JavaScript `validateExecutablePackage`.
pub fn validate_executable_package(repo_path: &Path, package: &Value) -> Result<()> {
    let mut checked_paths = HashSet::new();
    for relative_path in string_array(package.get("data")) {
        maybe_check_write_unit(repo_path, relative_path, &mut checked_paths)?;
    }
    for relative_path in provenance_files(package).iter().filter_map(Value::as_str) {
        maybe_check_write_unit(repo_path, relative_path, &mut checked_paths)?;
    }

    let schemas = schema_paths(package);
    for relative_path in string_array(
        package
            .get("assembly")
            .and_then(|assembly| assembly.get("loadOrder")),
    ) {
        if relative_path.ends_with(".tql") && !schemas.contains(relative_path) {
            maybe_check_write_unit(repo_path, relative_path, &mut checked_paths)?;
        }
    }

    if let Some(plans) = package
        .get("migration")
        .and_then(|migration| migration.get("plans"))
        .and_then(Value::as_array)
    {
        for plan in plans {
            if let Some(phases) = plan.get("phases").and_then(Value::as_array) {
                for phase in phases {
                    if let Some(units) = phase.get("units").and_then(Value::as_array) {
                        for unit in units {
                            if unit.get("kind").and_then(Value::as_str) == Some("write") {
                                if let Some(relative_path) =
                                    unit.get("path").and_then(Value::as_str)
                                {
                                    maybe_check_write_unit(
                                        repo_path,
                                        relative_path,
                                        &mut checked_paths,
                                    )?;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn write(root: &Path, relative_path: &str, contents: &str) {
        let path = root.join(relative_path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn fixture(package: &Value, files: &[(&str, String)]) -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        for (relative_path, contents) in files {
            write(directory.path(), relative_path, contents);
        }
        write(
            directory.path(),
            "package.json",
            &format!("{}\n", serde_json::to_string_pretty(package).unwrap()),
        );
        directory
    }

    fn repeated_put_file(count: usize, payload_size: usize) -> String {
        (0..count)
            .map(|index| {
                format!(
                    "put $r{index} isa SchemaResource,\n  has docKey \"doc-{index}\",\n  has definition \"{}\";",
                    "x".repeat(payload_size)
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    fn validation_message(root: &Path, package: &Value) -> String {
        match validate_executable_package(root, package) {
            Err(Error::Version(message)) => message,
            other => panic!("expected a version error, got {other:?}"),
        }
    }

    #[test]
    fn split_groups_put_statements_deterministically() {
        assert_eq!(
            split_executable_blocks(
                "put $a isa Thing, has name \"a\";\n\nput $b isa Thing, has name \"b\";\n"
            ),
            vec![
                "put $a isa Thing, has name \"a\";",
                "put $b isa Thing, has name \"b\";"
            ]
        );
    }

    #[test]
    fn split_handles_empty_comments_and_crlf() {
        assert!(split_executable_blocks(" \n\t").is_empty());
        assert!(split_executable_blocks("# only\r\n  # comments").is_empty());
        assert_eq!(
            split_executable_blocks("# header\r\nput $a isa Thing;\r\n\r\nput ($a) isa link;\r\n"),
            vec!["put $a isa Thing;\n\nput ($a) isa link;"]
        );
    }

    #[test]
    fn split_uses_paragraphs_for_non_put_writes() {
        assert_eq!(
            split_executable_blocks(
                "# ignored\nmatch\n  $x isa Thing;\ninsert $y isa Other;\n\n# ignored too\ndelete $x;"
            ),
            vec![
                "match\n  $x isa Thing;\ninsert $y isa Other;",
                "delete $x;"
            ]
        );
    }

    #[test]
    fn resolve_preambles_orders_recursive_dependencies() {
        let groups = vec![
            PutGroup {
                variable: Some("module".to_string()),
                statements: vec![
                    "put $module isa OntologyModule, has moduleName \"fixture\";".to_string(),
                ],
            },
            PutGroup {
                variable: Some("version".to_string()),
                statements: vec![
                    "put $version isa OntologyModuleVersion, has moduleVersion \"1.0.0\";"
                        .to_string(),
                    "put (version: $version, module: $module) isa ontologyModuleVersionOf;"
                        .to_string(),
                ],
            },
            PutGroup {
                variable: Some("artifact".to_string()),
                statements: vec![
                    "put $artifact isa SourceArtifactRecord, has sourcePath \"a\";".to_string(),
                    "put (version: $version, sourceArtifact: $artifact) isa ontologyModuleVersionBasedOnSourceArtifact;".to_string(),
                ],
            },
        ];
        assert_eq!(resolve_preamble_indexes(&[2], &groups), vec![0, 1]);
    }

    #[test]
    fn validate_rejects_empty_write_units_with_exact_message() {
        let directory = fixture(
            &json!({ "data": ["data/empty.tql"] }),
            &[("data/empty.tql", " \n".to_string())],
        );
        assert_eq!(
            validation_message(directory.path(), &json!({ "data": ["data/empty.tql"] })),
            "write unit 'data/empty.tql' is empty"
        );
    }

    #[test]
    fn validate_rejects_comment_only_units_with_exact_message() {
        let package = json!({ "data": ["data/comments.tql"] });
        let directory = fixture(
            &package,
            &[(
                "data/comments.tql",
                "# no query\n# still no query\n".to_string(),
            )],
        );
        assert_eq!(
            validation_message(directory.path(), &package),
            "write unit 'data/comments.tql' does not contain executable blocks"
        );
    }

    #[test]
    fn validate_rejects_too_many_blocks_with_exact_message() {
        let package = json!({ "data": ["data/blocks.tql"] });
        let text = (0..51)
            .map(|index| format!("match $x{index} isa Thing;\ninsert $y{index} isa Other;"))
            .collect::<Vec<_>>()
            .join("\n\n");
        let directory = fixture(&package, &[("data/blocks.tql", text)]);
        assert_eq!(
            validation_message(directory.path(), &package),
            "write unit 'data/blocks.tql' contains 51 executable blocks; max is 50"
        );
    }

    #[test]
    fn validate_rejects_oversized_units_with_exact_message() {
        let package = json!({ "data": ["data/large.tql"] });
        let text = format!("insert {};", "x".repeat(MAX_WRITE_UNIT_CHARS));
        let length = js_len(text.trim());
        let directory = fixture(&package, &[("data/large.tql", text)]);
        assert_eq!(
            validation_message(directory.path(), &package),
            format!(
                "write unit 'data/large.tql' is {length} chars; max is 50000. Publish sharded apply units instead of a single large query."
            )
        );
    }

    #[test]
    fn validate_preserves_data_before_provenance_error_order() {
        let package = json!({
            "data": ["data/empty.tql"],
            "provenance": { "files": ["data/comments.tql"] }
        });
        let directory = fixture(
            &package,
            &[
                ("data/empty.tql", String::new()),
                ("data/comments.tql", "# comment".to_string()),
            ],
        );
        assert_eq!(
            validation_message(directory.path(), &package),
            "write unit 'data/empty.tql' is empty"
        );
    }

    #[test]
    fn prepare_rewrites_oversized_files_and_updates_manifest() {
        let large_docs = repeated_put_file(180, 384);
        assert!(js_len(&large_docs) > MAX_WRITE_UNIT_CHARS);
        let package = json!({
            "name": "fixture",
            "displayName": "fixture",
            "version": "1.0.0",
            "schemas": [{ "name": "fixture", "file": "schema/fixture.tql" }],
            "data": ["data/docs.tql"],
            "provenance": {
                "files": ["data/provenance.tql"],
                "manifest": "manifests/fixture.package-manifest.json"
            },
            "assembly": {
                "loadOrder": ["schema/fixture.tql", "data/docs.tql", "data/provenance.tql"],
                "generatedArtifacts": []
            },
            "migration": {
                "plans": [{
                    "phases": [{
                        "units": [{ "kind": "write", "path": "migrations/v0.9.0-to-v1.0.0.tql" }]
                    }]
                }]
            }
        });
        let manifest = json!({
            "upstream": {
                "sourceArtifacts": [
                    { "path": "package.json", "sha256": "stale" },
                    { "path": "schema/fixture.tql", "sha256": "stale" },
                    { "path": "data/docs.tql", "sha256": "stale" },
                    { "path": "data/provenance.tql", "sha256": "stale" }
                ]
            },
            "artifacts": []
        });
        let directory = fixture(
            &package,
            &[
                ("schema/fixture.tql", "define entity Thing;\n".to_string()),
                ("data/docs.tql", large_docs.clone()),
                (
                    "data/provenance.tql",
                    "put $p isa SchemaResource, has docKey \"prov\";\n".to_string(),
                ),
                (
                    "manifests/fixture.package-manifest.json",
                    format!("{}\n", serde_json::to_string_pretty(&manifest).unwrap()),
                ),
                ("migrations/v0.9.0-to-v1.0.0.tql", large_docs),
            ],
        );

        let prepared = prepare_executable_package(directory.path(), None).unwrap();
        let data = prepared["data"].as_array().unwrap();
        assert!(data.len() > 1);
        assert!(data.iter().all(|path| {
            path.as_str()
                .unwrap()
                .starts_with("generated/apply-units/data/docs/")
        }));
        for path in data.iter().filter_map(Value::as_str) {
            let text = fs::read_to_string(directory.path().join(path)).unwrap();
            assert!(text.contains("# manifest: manifests/fixture.package-manifest.json"));
        }
        assert!(prepared["migration"]["plans"][0]["phases"][0]["units"]
            .as_array()
            .unwrap()
            .iter()
            .all(|unit| unit["path"]
                .as_str()
                .unwrap()
                .starts_with("generated/apply-units/migrations/v0.9.0-to-v1.0.0/")));

        let updated_manifest = read_json(
            &directory
                .path()
                .join("manifests/fixture.package-manifest.json"),
        )
        .unwrap();
        assert!(updated_manifest["artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|artifact| artifact["path"] == data[0]));
        let package_hash = hash_file(directory.path(), "package.json").unwrap();
        let recorded_hash = updated_manifest["upstream"]["sourceArtifacts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|artifact| artifact["path"] == "package.json")
            .unwrap()["sha256"]
            .as_str()
            .unwrap();
        assert_eq!(recorded_hash, package_hash);
    }

    #[test]
    fn prepare_preserves_existing_generated_units_without_nesting() {
        let package = json!({
            "data": ["generated/apply-units/data/docs/0001.tql"],
            "manifests": ["manifests/fixture.package-manifest.json"],
            "provenance": { "manifest": "manifests/fixture.package-manifest.json" },
            "assembly": {
                "loadOrder": ["schema/fixture.tql", "generated/apply-units/data/docs/0001.tql"],
                "generatedArtifacts": ["generated/apply-units/data/docs/0001.tql"]
            },
            "schemas": [{ "name": "fixture", "file": "schema/fixture.tql" }]
        });
        let directory = fixture(
            &package,
            &[
                ("schema/fixture.tql", "define entity Thing;\n".to_string()),
                (
                    "generated/apply-units/data/docs/0001.tql",
                    "# Generated executable apply unit from data/docs.tql\n\nput $r isa Thing;\n"
                        .to_string(),
                ),
                (
                    "manifests/fixture.package-manifest.json",
                    "{\n  \"upstream\": { \"sourceArtifacts\": [] },\n  \"artifacts\": []\n}\n"
                        .to_string(),
                ),
            ],
        );

        let prepared = prepare_executable_package(directory.path(), None).unwrap();
        assert_eq!(
            prepared["data"],
            json!(["generated/apply-units/data/docs/0001.tql"])
        );
        assert!(directory
            .path()
            .join("generated/apply-units/data/docs/0001.tql")
            .exists());
        assert!(!directory
            .path()
            .join("generated/apply-units/generated/apply-units/data/docs/0001.tql")
            .exists());
    }

    #[test]
    fn chunk_blocks_uses_javascript_utf16_lengths() {
        let blocks = vec!["😀".to_string()];
        let error = chunk_blocks(
            &blocks,
            PrepareExecutablePackageOptions {
                max_chars: 1,
                max_blocks: MAX_WRITE_UNIT_BLOCKS,
            },
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "version error: write block exceeds safe size limit (2 chars > 1) and must be split at the source"
        );
    }
}
