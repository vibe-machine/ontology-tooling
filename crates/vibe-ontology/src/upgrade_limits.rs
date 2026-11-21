//! Rules an upgrade (`migrations/`) must meet so TypeDB and the app can run it.
//!
//! The app (OneWorkspace Lingo, `OntologyViewModel+TypeDB.swift`
//! `applyMigrationDiff`) sends a `migrations/v<from>-to-v<to>.tql` file to
//! TypeDB as **one** query. In one query every stage runs once per row the
//! stages before it produced, so independent `match` changes chained in one
//! file multiply: 16 records with one path times 3 with another times … and
//! the query never finishes (seen as `CXN07`, the transaction closed). A
//! query is also limited to [`MAX_PIPELINE_STAGES`] stages (`QEX18`).
//!
//! So an upgrade is written as ordered units
//! `migrations/v<from>-to-v<to>/NNNN.tql`, one transaction each: a unit holds
//! either put/update groups only, or exactly one `match` change; a `match`
//! must select its record by a `@key` (or `@unique`) attribute; and each unit
//! meets the data write-unit limits.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::executable_package::assert_safe_write_unit;

/// TypeDB's maximum number of stages in one query pipeline (error `QEX18`).
pub const MAX_PIPELINE_STAGES: usize = 1000;

const STAGE_KEYWORDS: [&str; 13] = [
    "match", "insert", "put", "delete", "update", "fetch", "reduce", "sort", "offset", "limit",
    "select", "require", "distinct",
];

fn invalid<S: Into<String>>(message: S) -> Error {
    Error::Version(message.into())
}

/// One top-level stage of a query: its keyword and its text (up to the next
/// stage).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stage {
    pub keyword: String,
    pub text: String,
}

/// The stages TypeDB sees when `text` is sent as one query: every stage
/// keyword (`match`, `insert`, `put`, `delete`, `update`, `fetch`, `reduce`,
/// `sort`, `offset`, `limit`, `select`, `require`, `distinct`) that begins a
/// clause at the top level. Comments, string literals and anything inside
/// brackets are skipped.
pub fn pipeline_stages(text: &str) -> Vec<Stage> {
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0;
    let mut depth: usize = 0;
    let mut clause_start = true;
    let mut starts: Vec<(String, usize)> = Vec::new();

    while index < chars.len() {
        let character = chars[index];
        match character {
            '#' => {
                while index < chars.len() && chars[index] != '\n' {
                    index += 1;
                }
                continue;
            }
            '"' | '\'' => {
                let quote = character;
                index += 1;
                while index < chars.len() && chars[index] != quote {
                    if chars[index] == '\\' {
                        index += 1;
                    }
                    index += 1;
                }
                clause_start = false;
            }
            '{' | '(' | '[' => {
                depth += 1;
                clause_start = false;
            }
            '}' | ')' | ']' => {
                depth = depth.saturating_sub(1);
                clause_start = false;
            }
            ';' => clause_start = true,
            character if character.is_whitespace() => {}
            character if character.is_ascii_alphabetic() || character == '_' => {
                let start = index;
                while index < chars.len()
                    && (chars[index].is_ascii_alphanumeric()
                        || chars[index] == '_'
                        || chars[index] == '-')
                {
                    index += 1;
                }
                let word: String = chars[start..index].iter().collect();
                if depth == 0 && clause_start && STAGE_KEYWORDS.contains(&word.as_str()) {
                    starts.push((word, start));
                }
                clause_start = false;
                continue;
            }
            _ => clause_start = false,
        }
        index += 1;
    }

    starts
        .iter()
        .enumerate()
        .map(|(position, (keyword, start))| {
            let end = starts
                .get(position + 1)
                .map_or(chars.len(), |(_, next)| *next);
            Stage {
                keyword: keyword.clone(),
                text: chars[*start..end].iter().collect(),
            }
        })
        .collect()
}

/// Counts the stages TypeDB sees when `text` is sent as one query.
pub fn count_pipeline_stages(text: &str) -> usize {
    pipeline_stages(text).len()
}

/// The `@key` / `@unique` attributes each type owns, from schema TypeQL, with
/// supertypes so an inherited key counts.
#[derive(Debug, Clone, Default)]
pub struct SchemaKeys {
    types: HashMap<String, (Option<String>, HashSet<String>)>,
}

fn type_statement_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"^(?:(?:define|redefine)\s+)?(?:entity|relation|attribute)\s+([A-Za-z][\w-]*)(?:\s+sub\s+([A-Za-z][\w-]*))?")
            .expect("type statement regex is valid")
    })
}

fn owns_key_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"owns\s+([A-Za-z][\w-]*)(?:\[\])?\s*([^,;]*)").expect("owns regex is valid")
    })
}

fn strip_comments(text: &str) -> String {
    text.lines()
        .map(|line| line.split('#').next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

impl SchemaKeys {
    /// Reads type definitions from schema TypeQL texts.
    pub fn from_schema_texts<S: AsRef<str>>(texts: &[S]) -> Self {
        let mut keys = Self::default();
        for text in texts {
            for statement in strip_comments(text.as_ref()).split(';') {
                let statement = statement.trim();
                let Some(captures) = type_statement_regex().captures(statement) else {
                    continue;
                };
                let entry = keys
                    .types
                    .entry(captures[1].to_string())
                    .or_insert((None, HashSet::new()));
                if let Some(parent) = captures.get(2) {
                    entry.0 = Some(parent.as_str().to_string());
                }
                for owns in owns_key_regex().captures_iter(statement) {
                    if owns[2].contains("@key") || owns[2].contains("@unique") {
                        entry.1.insert(owns[1].to_string());
                    }
                }
            }
        }
        keys
    }

    /// Reads the package's own schema files and those of its dependencies
    /// (sibling `ontology-<name>` repositories), skipping any it cannot read.
    pub fn for_package(repo_path: &Path, package: &Value) -> Self {
        let mut texts = Vec::new();
        let mut seen = HashSet::new();
        collect_schema_texts(repo_path, package, &mut texts, &mut seen);
        Self::from_schema_texts(&texts)
    }

    /// `Some(true)` when `attribute` is a `@key`/`@unique` attribute of
    /// `type_name` or a supertype, `Some(false)` when it is not, `None` when
    /// the type is not defined in the schemas read.
    pub fn is_key(&self, type_name: &str, attribute: &str) -> Option<bool> {
        let mut current = Some(type_name.to_string());
        let mut visited = HashSet::new();
        let mut known = false;
        while let Some(name) = current {
            if !visited.insert(name.clone()) {
                break;
            }
            let Some((parent, attributes)) = self.types.get(&name) else {
                break;
            };
            known = true;
            if attributes.contains(attribute) {
                return Some(true);
            }
            current = parent.clone();
        }
        known.then_some(false)
    }
}

fn collect_schema_texts(
    repo_path: &Path,
    package: &Value,
    texts: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    let name = package
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if !seen.insert(name) {
        return;
    }
    for schema in package
        .get("schemas")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(file) = schema.get("file").and_then(Value::as_str) {
            if let Ok(text) = fs::read_to_string(repo_path.join(file)) {
                texts.push(text);
            }
        }
    }
    for dependency in package
        .get("dependencies")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(dependency_name) = dependency.get("name").and_then(Value::as_str) else {
            continue;
        };
        let Some(dependency_root) = repo_path
            .parent()
            .map(|parent| parent.join(format!("ontology-{dependency_name}")))
        else {
            continue;
        };
        let Ok(text) = fs::read_to_string(dependency_root.join("package.json")) else {
            continue;
        };
        if let Ok(dependency_package) = serde_json::from_str::<Value>(&text) {
            collect_schema_texts(&dependency_root, &dependency_package, texts, seen);
        }
    }
}

fn isa_statement_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"\$([\w-]+)\s+isa\s+([A-Za-z][\w-]*)([^;]*)").expect("isa regex is valid")
    })
}

fn literal_has_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r#"has\s+([A-Za-z][\w-]*)\s+("(?:[^"\\]|\\.)*"|[^\s,;$"][^\s,;]*)"#)
            .expect("literal has regex is valid")
    })
}

/// The text with everything inside `{ … }` (negations, disjunctions, `try`,
/// which produce no rows for the stages after them) removed, and every string
/// literal emptied (`"…"` becomes `""`), so text inside a value (a stored
/// query, say) is never read as a statement.
fn top_level_only(text: &str) -> String {
    let mut depth: usize = 0;
    let mut result = String::new();
    let mut in_string = false;
    let mut escaped = false;
    for character in text.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
                if depth == 0 {
                    result.push('"');
                }
            }
            continue;
        }
        match character {
            '"' => {
                in_string = true;
                if depth == 0 {
                    result.push('"');
                }
            }
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            _ if depth == 0 => result.push(character),
            _ => {}
        }
    }
    result
}

/// Every `$var isa Type` in a `match` stage whose type is known and which is
/// not selected by one of its `@key`/`@unique` attributes: `(var, type,
/// literal attributes)`.
fn unanchored_matches(match_text: &str, keys: &SchemaKeys) -> Vec<(String, String, Vec<String>)> {
    let mut found = Vec::new();
    for captures in isa_statement_regex().captures_iter(&top_level_only(match_text)) {
        let (variable, type_name, rest) = (&captures[1], &captures[2], &captures[3]);
        let attributes: Vec<String> = literal_has_regex()
            .captures_iter(rest)
            .map(|has| has[1].to_string())
            .collect();
        let anchored = attributes
            .iter()
            .any(|attribute| keys.is_key(type_name, attribute) == Some(true));
        if !anchored && keys.is_key(type_name, "").is_some() {
            found.push((variable.to_string(), type_name.to_string(), attributes));
        }
    }
    found
}

/// Checks one upgrade unit (`migrations/v<from>-to-v<to>/NNNN.tql`), which
/// runs as one query in its own transaction.
pub fn check_upgrade_unit(relative_path: &str, text: &str, keys: &SchemaKeys) -> Result<()> {
    assert_safe_write_unit(relative_path, text)?;
    let stages = pipeline_stages(text);
    if stages.len() > MAX_PIPELINE_STAGES {
        return Err(invalid(format!(
            "upgrade unit '{relative_path}' is one query of {} stages; TypeDB runs at most \
             {MAX_PIPELINE_STAGES} stages in one query (QEX18)",
            stages.len()
        )));
    }

    // In one query a variable is one concept: declaring it with two types is
    // refused by TypeDB (INF11), as lingo's generated v1.0.0-to-v1.0.1.tql
    // was for `$module` (a SchemaModule and an OntologyModule).
    let mut declared: Vec<(String, String)> = Vec::new();
    for captures in isa_statement_regex().captures_iter(&top_level_only(text)) {
        let (variable, type_name) = (captures[1].to_string(), captures[2].to_string());
        if let Some((_, first)) = declared.iter().find(|(name, _)| *name == variable) {
            if *first != type_name {
                return Err(invalid(format!(
                    "upgrade unit '{relative_path}' declares ${variable} as both {first} and \
                     {type_name}. In one query a variable is one concept, so TypeDB refuses it \
                     (INF11). Give each its own variable, or put them in separate units."
                )));
            }
        } else {
            declared.push((variable, type_name));
        }
    }

    let matches = stages
        .iter()
        .filter(|stage| stage.keyword == "match")
        .count();
    if matches > 1 {
        return Err(invalid(format!(
            "upgrade unit '{relative_path}' chains {matches} match stages in one query. Each \
             stage runs once for every row the stages before it produced, so chained matches \
             multiply (a match on 16 records, then one on 3, runs the rest 48 times) and can \
             touch records the change did not mean. Put each match/delete/insert change in a \
             unit of its own."
        )));
    }
    if let Some(position) = stages.iter().position(|stage| stage.keyword == "match") {
        if stages[position + 1..]
            .iter()
            .any(|stage| stage.keyword == "put")
        {
            return Err(invalid(format!(
                "upgrade unit '{relative_path}' has a put after its match. Each stage runs once \
                 for every row the match produced, so a change after a match repeats for every \
                 matched record. Put the match/delete/insert change in a unit of its own."
            )));
        }
        if let Some((variable, type_name, attributes)) =
            unanchored_matches(&stages[position].text, keys)
                .into_iter()
                .next()
        {
            let selected_by = if attributes.is_empty() {
                "no attribute value".to_string()
            } else {
                attributes.join(", ")
            };
            return Err(invalid(format!(
                "upgrade unit '{relative_path}' matches ${variable} isa {type_name} by \
                 {selected_by}, none of which is a @key or @unique attribute of {type_name}, so \
                 it selects every such {type_name} record (other packages' and other versions' \
                 too), not one. Match the record by its key."
            )));
        }
    }
    Ok(())
}

/// Splits schema TypeQL into its text without `fun` definitions and the
/// definitions themselves. A definition starts at a line beginning `fun ` and
/// ends at its `return …;` line.
pub fn split_functions(text: &str) -> (String, Vec<String>) {
    let mut kept = Vec::new();
    let mut functions = Vec::new();
    let mut current: Option<Vec<&str>> = None;
    for line in text.lines() {
        if let Some(block) = current.as_mut() {
            block.push(line);
            let trimmed = line.trim();
            if trimmed.starts_with("return") && trimmed.ends_with(';') {
                functions.push(block.join("\n"));
                current = None;
            }
        } else if line.starts_with("fun ") {
            current = Some(vec![line]);
        } else {
            kept.push(line);
        }
    }
    if let Some(block) = current {
        kept.extend(block);
    }
    (kept.join("\n"), functions)
}

/// The name of a `fun` definition.
pub fn function_name(definition: &str) -> Option<&str> {
    definition
        .trim_start()
        .strip_prefix("fun ")
        .and_then(|rest| rest.split('(').next())
        .map(str::trim)
}

/// Parses `v<from>-to-v<to>` (the app's upgrade-file name, without `.tql`)
/// into its target version.
fn upgrade_target(stem: &str) -> Option<&str> {
    fn is_version(text: &str) -> bool {
        let parts: Vec<&str> = text.split('.').collect();
        parts.len() == 3
            && parts
                .iter()
                .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
    }
    stem.strip_prefix('v')
        .and_then(|rest| rest.split_once("-to-v"))
        .filter(|(from, to)| is_version(from) && is_version(to))
        .map(|(_, to)| to)
}

/// Validates the upgrades in `<repo>/migrations`:
///
/// - a single-file upgrade `v<from>-to-v<to>.tql` to the package's current
///   version is refused (the app runs it as one query); single files to
///   earlier versions are released history and are not re-checked;
/// - every unit of every `v<from>-to-v<to>/` directory must pass
///   [`check_upgrade_unit`].
///
/// Other files (preflight/verify assertions, hand-written plan units) are left
/// to the migration contract.
pub fn validate_upgrade_files(repo_path: &Path) -> Result<()> {
    let migrations = repo_path.join("migrations");
    let Ok(entries) = fs::read_dir(&migrations) else {
        return Ok(());
    };
    let package: Value = fs::read_to_string(repo_path.join("package.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null);
    let version = package
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let keys = SchemaKeys::for_package(repo_path, &package);
    let mut entries: Vec<_> = entries.filter_map(std::result::Result::ok).collect();
    entries.sort_by_key(fs::DirEntry::file_name);

    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if path.is_file() {
            let Some(stem) = name.strip_suffix(".tql") else {
                continue;
            };
            if upgrade_target(stem) == Some(version.as_str()) {
                return Err(invalid(format!(
                    "upgrade file 'migrations/{name}' is a single-file upgrade to this release \
                     (v{version}). The app sends it as one query, so all of its changes run in \
                     one pipeline and multiply each other's rows. Release with the current `ont \
                     release`, which writes one change per unit to migrations/{stem}/NNNN.tql."
                )));
            }
        } else if path.is_dir() && upgrade_target(&name).is_some() {
            let mut units: Vec<_> = fs::read_dir(&path)
                .map_err(|error| invalid(format!("failed to read {}: {error}", path.display())))?
                .filter_map(std::result::Result::ok)
                .map(|unit| unit.path())
                .filter(|unit| unit.extension().is_some_and(|ext| ext == "tql"))
                .collect();
            units.sort();
            if units.is_empty() {
                return Err(invalid(format!(
                    "upgrade directory 'migrations/{name}' contains no .tql units"
                )));
            }
            for unit in units {
                let unit_name = unit
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let text = read(&unit)?;
                check_upgrade_unit(&format!("migrations/{name}/{unit_name}"), &text, &keys)?;
            }
        }
    }
    Ok(())
}

fn read(path: &Path) -> Result<String> {
    fs::read_to_string(path)
        .map_err(|error| invalid(format!("failed to read {}: {error}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCHEMA: &str = r"
define
  entity SchemaMetaData @abstract;
  entity OntologyModuleVersion sub SchemaMetaData,
    owns moduleVersionKey @key,
    owns moduleVersion;
  entity SourceArtifactRecord sub SchemaMetaData,
    owns sourcePath,
    owns sourceSha256; # owns fake @key in a comment
  entity Seed, owns seedKey @key, owns label;
  entity SpecialSeed sub Seed;
";

    fn keys() -> SchemaKeys {
        SchemaKeys::from_schema_texts(&[SCHEMA])
    }

    #[test]
    fn counts_top_level_stages_only() {
        let text = r#"
# put this is a comment: match insert
put $a isa A, has name "match; insert";
put (x: $a) isa r;
match
  $b isa B, has name "b";
  not { $b has flag true; };
  { $b has n 1; } or { $b has n 2; };
delete
  has $old of $b;
insert
  $b has name "c";
"#;
        assert_eq!(count_pipeline_stages(text), 5);
        let keywords: Vec<String> = pipeline_stages(text)
            .into_iter()
            .map(|stage| stage.keyword)
            .collect();
        assert_eq!(keywords, ["put", "put", "match", "delete", "insert"]);
    }

    #[test]
    fn counts_fetch_reduce_and_modifiers() {
        let text = "match $x isa T;\nsort $x;\nlimit 3;\nfetch { \"x\": $x };";
        assert_eq!(count_pipeline_stages(text), 4);
        assert_eq!(
            count_pipeline_stages("match $x isa T;\nreduce $c = count;"),
            2
        );
    }

    #[test]
    fn reads_keys_with_inheritance() {
        let keys = keys();
        assert_eq!(keys.is_key("Seed", "seedKey"), Some(true));
        assert_eq!(keys.is_key("SpecialSeed", "seedKey"), Some(true));
        assert_eq!(keys.is_key("Seed", "label"), Some(false));
        assert_eq!(
            keys.is_key("SourceArtifactRecord", "sourcePath"),
            Some(false)
        );
        assert_eq!(keys.is_key("SourceArtifactRecord", "fake"), Some(false));
        assert_eq!(keys.is_key("Unknown", "x"), None);
    }

    #[test]
    fn a_unit_with_two_matches_is_rejected_with_an_exact_message() {
        let text = "match $a isa Seed, has seedKey \"a\", has label $o;\ndelete has $o of $a;\n\
                    match $b isa Seed, has seedKey \"b\", has label $p;\ndelete has $p of $b;\n";
        let error = check_upgrade_unit("migrations/v1.0.0-to-v1.0.1/0001.tql", text, &keys())
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(
                "upgrade unit 'migrations/v1.0.0-to-v1.0.1/0001.tql' chains 2 match stages in one \
                 query. Each stage runs once for every row the stages before it produced, so \
                 chained matches multiply (a match on 16 records, then one on 3, runs the rest 48 \
                 times) and can touch records the change did not mean. Put each \
                 match/delete/insert change in a unit of its own."
            ),
            "{error}"
        );
    }

    #[test]
    fn a_variable_declared_with_two_types_is_rejected_with_an_exact_message() {
        let text = "put $module isa SchemaModule,\n  has moduleKey \"lingo\";\n\n\
                    put $module isa OntologyModule,\n  has moduleRepoUrl \"u\";\n";
        let error = check_upgrade_unit("migrations/v1.0.0-to-v1.0.1/0001.tql", text, &keys())
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(
                "upgrade unit 'migrations/v1.0.0-to-v1.0.1/0001.tql' declares $module as both \
                 SchemaModule and OntologyModule. In one query a variable is one concept, so \
                 TypeDB refuses it (INF11). Give each its own variable, or put them in separate \
                 units."
            ),
            "{error}"
        );
        // A query stored as a value is not read as statements.
        check_upgrade_unit(
            "migrations/v1.0.0-to-v1.0.1/0001.tql",
            "put $q isa Seed, has seedKey \"q\", has label \"match $d isa A; $d isa B;\";\n",
            &keys(),
        )
        .unwrap();
        // The same variable with the same type twice is fine.
        check_upgrade_unit(
            "migrations/v1.0.0-to-v1.0.1/0001.tql",
            "put $a isa Seed, has seedKey \"a\";\nput $a isa Seed, has seedKey \"a\";\n",
            &keys(),
        )
        .unwrap();
    }

    #[test]
    fn a_put_after_a_match_is_rejected() {
        let text = "match $a isa Seed, has seedKey \"a\";\ninsert $a has label \"x\";\n\
                    put $b isa Seed, has seedKey \"b\";\n";
        let error = check_upgrade_unit("migrations/v1.0.0-to-v1.0.1/0001.tql", text, &keys())
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(
                "upgrade unit 'migrations/v1.0.0-to-v1.0.1/0001.tql' has a put after its match."
            ),
            "{error}"
        );
    }

    #[test]
    fn a_match_not_selected_by_a_key_is_rejected_with_an_exact_message() {
        let text = "match\n  $s isa SourceArtifactRecord,\n    has sourcePath \"package.json\",\n    \
                    has sourceSha256 $old;\ndelete\n  has $old of $s;\ninsert\n  $s has sourceSha256 \"x\";\n";
        let error = check_upgrade_unit("migrations/v1.0.2-to-v1.1.0/0002.tql", text, &keys())
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(
                "upgrade unit 'migrations/v1.0.2-to-v1.1.0/0002.tql' matches $s isa \
                 SourceArtifactRecord by sourcePath, none of which is a @key or @unique attribute \
                 of SourceArtifactRecord, so it selects every such SourceArtifactRecord record \
                 (other packages' and other versions' too), not one. Match the record by its key."
            ),
            "{error}"
        );
        let anchored =
            "match\n  $s isa SpecialSeed,\n    has seedKey \"a\",\n    has label $old;\n\
                        not { $t isa Seed, has label \"x\"; };\ndelete\n  has $old of $s;\n";
        check_upgrade_unit("migrations/v1.0.2-to-v1.1.0/0001.tql", anchored, &keys()).unwrap();
        // A type the schemas do not define cannot be judged and is let through.
        check_upgrade_unit(
            "migrations/v1.0.2-to-v1.1.0/0001.tql",
            "match $x isa Elsewhere, has name \"a\";\ndelete $x;\n",
            &keys(),
        )
        .unwrap();
    }

    #[test]
    fn upgrade_unit_is_held_to_write_unit_limits() {
        let text: String = (0..51)
            .map(|index| format!("put $x{index} isa T, has name \"{index}\";\n\n"))
            .collect();
        let error = check_upgrade_unit("migrations/v1.0.0-to-v1.0.1/0001.tql", &text, &keys())
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(
                "write unit 'migrations/v1.0.0-to-v1.0.1/0001.tql' contains 51 executable blocks; max is 50"
            ),
            "{error}"
        );
    }

    #[test]
    fn refuses_a_single_file_upgrade_to_this_release_only() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::write(
            root.join("package.json"),
            r#"{"name": "p", "version": "1.1.0"}"#,
        )
        .unwrap();
        let migrations = root.join("migrations");
        fs::create_dir_all(migrations.join("verify")).unwrap();
        fs::create_dir_all(migrations.join("v1.0.2-to-v1.1.0")).unwrap();
        // Released history is not re-checked.
        fs::write(
            migrations.join("v1.0.0-to-v1.0.1.tql"),
            "match $x isa T;\n".repeat(5),
        )
        .unwrap();
        fs::write(
            migrations.join("verify/assert-v1.1.0-module-version.tql"),
            "match $x isa T;\n".repeat(3),
        )
        .unwrap();
        fs::write(
            migrations.join("v1.0.2-to-v1.1.0/0001.tql"),
            "put $x isa T, has name \"a\";\n",
        )
        .unwrap();
        validate_upgrade_files(root).unwrap();

        fs::write(migrations.join("v1.0.2-to-v1.1.0.tql"), "put $x isa T;\n").unwrap();
        let error = validate_upgrade_files(root).unwrap_err().to_string();
        assert!(
            error.contains(
                "upgrade file 'migrations/v1.0.2-to-v1.1.0.tql' is a single-file upgrade to this \
                 release (v1.1.0). The app sends it as one query, so all of its changes run in \
                 one pipeline and multiply each other's rows. Release with the current `ont \
                 release`, which writes one change per unit to migrations/v1.0.2-to-v1.1.0/NNNN.tql."
            ),
            "{error}"
        );
    }

    #[test]
    fn validates_every_unit_directory() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::write(
            root.join("package.json"),
            r#"{"name": "p", "version": "1.1.0"}"#,
        )
        .unwrap();
        let units = root.join("migrations/v1.0.0-to-v1.0.1");
        fs::create_dir_all(&units).unwrap();
        fs::write(
            units.join("0001.tql"),
            "match $a isa T;\ndelete $a;\nmatch $b isa T;\ndelete $b;\n",
        )
        .unwrap();
        let error = validate_upgrade_files(root).unwrap_err().to_string();
        assert!(
            error.contains("'migrations/v1.0.0-to-v1.0.1/0001.tql' chains 2 match stages"),
            "{error}"
        );
    }

    #[test]
    fn splits_function_definitions_from_types() {
        let text = "define\nentity A;\n\nfun f($a: A) -> { A }:\n  match\n    $a isa A;\n  return { $a };\n\nentity B;\n";
        let (types, functions) = split_functions(text);
        assert_eq!(types, "define\nentity A;\n\n\nentity B;");
        assert_eq!(functions.len(), 1);
        assert_eq!(function_name(&functions[0]), Some("f"));
    }

    #[test]
    fn recognises_only_the_apps_upgrade_names() {
        assert_eq!(upgrade_target("v0.1.9-to-v1.0.0"), Some("1.0.0"));
        assert_eq!(upgrade_target("v0.1.4-to-v0.1.7-schema"), None);
        assert_eq!(upgrade_target("assert-v1.0.0-module-version"), None);
    }
}
