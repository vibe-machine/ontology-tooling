//! Competency questions: the questions a package's model must be able to
//! answer, each with a read query that proves it.
//!
//! A package declares them in `package.json`:
//!
//! - `"competencyQuestions": "competency/questions.json"` — a path, relative to
//!   the repository, to the questions file; or
//! - `"competencyQuestions": {"waived": "<non-empty reason>"}` — for a package
//!   that deliberately has none.
//!
//! The questions file (format version 1) lists sample-data `fixtures` and the
//! `questions`, each with an `id`, `number`, `group`, `text` and a `query` file
//! holding exactly one read query.
//!
//! This module owns three things:
//!
//! 1. the **static check** ([`validate_competency_questions`]), part of package
//!    validation;
//! 2. the **run** ([`run_competency_questions`]), which loads the package and its
//!    dependencies into a throwaway TypeDB and runs every question's query; a
//!    question passes when its query runs and returns at least one row;
//! 3. the **release gate** ([`check_release_gate`]): `competency/last-run.json`
//!    must exist and its `questionsDigest` must match the current files.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use typedb_driver::answer::QueryAnswer;
use typedb_driver::{
    Addresses, Credentials, DriverOptions, DriverTlsConfig, TransactionType, TypeDBDriver,
};

use crate::apply::{apply_schema, apply_write, create_database, TypeDbTarget};
use crate::error::{Error, Result};

/// The `package.json` field that declares the questions.
pub const FIELD: &str = "competencyQuestions";
/// The only questions-file format this tooling understands.
pub const FORMAT_VERSION: u64 = 1;
/// Where the last successful run is recorded (relative to the repository).
pub const LAST_RUN_PATH: &str = "competency/last-run.json";
/// Where the rendered questions document lives (relative to the repository).
pub const DOC_PATH: &str = "docs/competency-questions.md";
/// Query files under this directory must each be used by a question or listed
/// in `package.json` `queries`.
pub const QUERIES_DIR: &str = "queries";

fn invalid<S: Into<String>>(message: S) -> Error {
    Error::Version(message.into())
}

/// What a package declares about its competency questions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompetencyDeclaration {
    /// The package has questions in `path` (relative to the repository).
    Declared { path: String, file: QuestionsFile },
    /// The package deliberately has none.
    Waived { reason: String },
}

/// A parsed, validated questions file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionsFile {
    pub fixtures: Vec<String>,
    pub questions: Vec<Question>,
}

/// One competency question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub id: String,
    pub number: u64,
    pub group: String,
    pub text: String,
    pub query: String,
    /// Short name of the query (optional; the app shows it as the query title).
    pub title: Option<String>,
    /// One or two plain sentences on how the query answers the question.
    pub explanation: Option<String>,
}

// ---------------------------------------------------------------------------
// Static check
// ---------------------------------------------------------------------------

/// Validates the package's competency-question declaration and, when questions
/// are declared, the questions file, its fixtures and query files, the
/// `queries/` directory (no orphans), and the rendered document (when present).
pub fn validate_competency_questions(
    repo_path: &Path,
    package: &Value,
) -> Result<CompetencyDeclaration> {
    let declaration = read_declaration(repo_path, package)?;
    if let CompetencyDeclaration::Declared { path, file } = &declaration {
        validate_no_orphan_queries(repo_path, package, file)?;
        validate_rendered_doc(repo_path, package, path, file)?;
        crate::stored_questions::validate_generated(repo_path, package, path, file)?;
    }
    Ok(declaration)
}

/// Reads and validates the declaration and questions file, without the
/// orphan-query and rendered-document checks.
pub fn read_declaration(repo_path: &Path, package: &Value) -> Result<CompetencyDeclaration> {
    match package.get(FIELD) {
        None | Some(Value::Null) => Err(invalid(format!(
            "package.json '{FIELD}' is required: set it to the questions file path \
             (\"competency/questions.json\") or to {{\"waived\": \"<reason>\"}} for a package \
             that deliberately has none"
        ))),
        Some(Value::String(path)) if !path.trim().is_empty() => {
            let file = read_questions_file(repo_path, path)?;
            Ok(CompetencyDeclaration::Declared {
                path: path.clone(),
                file,
            })
        }
        Some(Value::Object(object)) => match object.get("waived").and_then(Value::as_str) {
            Some(reason) if !reason.trim().is_empty() && object.len() == 1 => {
                Ok(CompetencyDeclaration::Waived {
                    reason: reason.trim().to_string(),
                })
            }
            _ => Err(invalid(format!(
                "package.json '{FIELD}' waiver must be {{\"waived\": \"<non-empty reason>\"}}"
            ))),
        },
        Some(_) => Err(invalid(format!(
            "package.json '{FIELD}' must be the questions file path (\"competency/questions.json\") \
             or {{\"waived\": \"<reason>\"}}"
        ))),
    }
}

fn read_questions_file(repo_path: &Path, path: &str) -> Result<QuestionsFile> {
    let full_path = joined(repo_path, path);
    if !full_path.is_file() {
        return Err(invalid(format!(
            "competency questions file not found: {path}"
        )));
    }
    let text = std::fs::read_to_string(&full_path)
        .map_err(|error| invalid(format!("failed to read {path}: {error}")))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| invalid(format!("invalid json in {path}: {error}")))?;
    let Some(object) = value.as_object() else {
        return Err(invalid(format!("{path}: must be a JSON object")));
    };

    if object.get("formatVersion").and_then(Value::as_u64) != Some(FORMAT_VERSION) {
        return Err(invalid(format!(
            "{path}: 'formatVersion' must be {FORMAT_VERSION}"
        )));
    }

    let fixtures = match object.get("fixtures") {
        None => Vec::new(),
        Some(Value::Array(entries)) => {
            let mut fixtures = Vec::new();
            for (index, entry) in entries.iter().enumerate() {
                let Some(fixture) = entry.as_str().filter(|value| !value.trim().is_empty()) else {
                    return Err(invalid(format!(
                        "{path}: fixture {} must be a non-empty path",
                        index + 1
                    )));
                };
                if !joined(repo_path, fixture).is_file() {
                    return Err(invalid(format!("{path}: fixture not found: {fixture}")));
                }
                fixtures.push(fixture.to_string());
            }
            fixtures
        }
        Some(_) => {
            return Err(invalid(format!(
                "{path}: 'fixtures' must be an array of paths"
            )))
        }
    };

    let Some(entries) = object
        .get("questions")
        .and_then(Value::as_array)
        .filter(|entries| !entries.is_empty())
    else {
        return Err(invalid(format!(
            "{path}: 'questions' must contain at least one question"
        )));
    };

    let mut questions = Vec::new();
    let mut ids = BTreeSet::new();
    let mut numbers: HashMap<u64, String> = HashMap::new();
    for (index, entry) in entries.iter().enumerate() {
        let Some(entry) = entry.as_object() else {
            return Err(invalid(format!(
                "{path}: question {} must be an object",
                index + 1
            )));
        };
        let Some(id) = non_empty(entry.get("id")) else {
            return Err(invalid(format!(
                "{path}: question {} 'id' must be a non-empty string",
                index + 1
            )));
        };
        if !ids.insert(id.to_string()) {
            return Err(invalid(format!("{path}: duplicate question id '{id}'")));
        }
        let Some(number) = entry
            .get("number")
            .and_then(Value::as_u64)
            .filter(|number| *number > 0)
        else {
            return Err(invalid(format!(
                "{path}: question '{id}' 'number' must be a positive integer"
            )));
        };
        if let Some(previous) = numbers.get(&number) {
            return Err(invalid(format!(
                "{path}: question '{id}' reuses number {number} (already used by '{previous}')"
            )));
        }
        numbers.insert(number, id.to_string());
        let Some(group) = non_empty(entry.get("group")) else {
            return Err(invalid(format!(
                "{path}: question '{id}' 'group' must be a non-empty string"
            )));
        };
        let Some(text) = non_empty(entry.get("text")) else {
            return Err(invalid(format!(
                "{path}: question '{id}' 'text' must be a non-empty string"
            )));
        };
        let Some(query) = non_empty(entry.get("query")) else {
            return Err(invalid(format!(
                "{path}: question '{id}' 'query' must be a non-empty path"
            )));
        };
        let query_path = joined(repo_path, query);
        if !query_path.is_file() {
            return Err(invalid(format!(
                "{path}: question '{id}' query file not found: {query}"
            )));
        }
        let query_text = std::fs::read_to_string(&query_path)
            .map_err(|error| invalid(format!("failed to read {query}: {error}")))?;
        if let Err(reason) = check_single_read_query(&query_text) {
            return Err(invalid(format!(
                "{path}: question '{id}' query {query} must hold exactly one read query: {reason}"
            )));
        }
        let optional_text = |field: &str| -> Result<Option<String>> {
            match entry.get(field) {
                None | Some(Value::Null) => Ok(None),
                Some(value) => match non_empty(Some(value)) {
                    Some(text) => Ok(Some(text.to_string())),
                    None => Err(invalid(format!(
                        "{path}: question '{id}' '{field}' must be a non-empty string when present"
                    ))),
                },
            }
        };
        let title = optional_text("title")?;
        let explanation = optional_text("explanation")?;
        questions.push(Question {
            id: id.to_string(),
            number,
            group: group.to_string(),
            text: text.to_string(),
            query: query.to_string(),
            title,
            explanation,
        });
    }

    Ok(QuestionsFile {
        fixtures,
        questions,
    })
}

fn non_empty(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

const WRITE_STAGES: &[&str] = &[
    "insert", "put", "delete", "update", "define", "undefine", "redefine",
];
const READ_STARTS: &[&str] = &["match", "with", "fetch"];

/// Checks that `text` holds exactly one read query: leading `#` comment lines
/// are allowed; the query must start with `match`, `with` or `fetch`, must not
/// contain a write or schema stage, and must not hold a console `end;`
/// terminator (which separates several queries).
pub fn check_single_read_query(text: &str) -> std::result::Result<(), String> {
    let words = query_words(text);
    let Some(first) = words.first() else {
        return Err("it is empty".to_string());
    };
    if !READ_STARTS.contains(&first.as_str()) {
        return Err(format!(
            "it starts with '{first}', not 'match', 'with' or 'fetch'"
        ));
    }
    if let Some(stage) = words
        .iter()
        .find(|word| WRITE_STAGES.contains(&word.as_str()))
    {
        return Err(format!("it holds a write or schema stage ('{stage}')"));
    }
    if words.iter().any(|word| word == "end") {
        return Err("it holds more than one query ('end;')".to_string());
    }
    Ok(())
}

/// The bare keyword-like words of a query: comments and string literals are
/// removed, and `$variables`, `?parameters` and words inside identifiers
/// (`type-name`, `a:b`) are skipped.
fn query_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    for line in text.lines() {
        let mut stripped = String::new();
        let mut in_string = false;
        let mut escaped = false;
        for character in line.chars() {
            if in_string {
                if escaped {
                    escaped = false;
                } else if character == '\\' {
                    escaped = true;
                } else if character == '"' {
                    in_string = false;
                }
                stripped.push(' ');
                continue;
            }
            match character {
                '"' => {
                    in_string = true;
                    stripped.push(' ');
                }
                '#' => break,
                _ => stripped.push(character),
            }
        }

        let mut current = String::new();
        let mut previous: Option<char> = None;
        let mut word_prefix: Option<char> = None;
        let flush = |current: &mut String, prefix: Option<char>, words: &mut Vec<String>| {
            if !current.is_empty() && !matches!(prefix, Some('$' | '?' | '-' | ':' | '_' | '.')) {
                words.push(std::mem::take(current));
            }
            current.clear();
        };
        for character in stripped.chars() {
            if character.is_ascii_alphanumeric() {
                if current.is_empty() {
                    word_prefix = previous;
                }
                current.push(character);
            } else if !current.is_empty() && matches!(character, '-' | '_' | ':' | '.') {
                // Part of an identifier such as `type-name`; not a keyword.
                current.clear();
                word_prefix = Some(character);
            } else {
                flush(&mut current, word_prefix, &mut words);
            }
            previous = Some(character);
        }
        flush(&mut current, word_prefix, &mut words);
    }
    words
}

fn validate_no_orphan_queries(
    repo_path: &Path,
    package: &Value,
    file: &QuestionsFile,
) -> Result<()> {
    let queries_dir = repo_path.join(QUERIES_DIR);
    if !queries_dir.is_dir() {
        return Ok(());
    }
    let mut used: BTreeSet<String> = file
        .questions
        .iter()
        .map(|question| normalize_relative(&question.query))
        .collect();
    if let Some(listed) = package.get("queries").and_then(Value::as_array) {
        used.extend(
            listed
                .iter()
                .filter_map(Value::as_str)
                .map(normalize_relative),
        );
    }
    let mut found = Vec::new();
    collect_tql_files(repo_path, &queries_dir, &mut found)?;
    found.sort();
    for relative in found {
        if !used.contains(&relative) {
            return Err(invalid(format!(
                "{relative} is not used by any competency question and is not listed in \
                 package.json 'queries'"
            )));
        }
    }
    Ok(())
}

fn collect_tql_files(repo_path: &Path, directory: &Path, found: &mut Vec<String>) -> Result<()> {
    let entries = std::fs::read_dir(directory).map_err(|source| Error::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| Error::Io {
            path: directory.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if path.is_dir() {
            collect_tql_files(repo_path, &path, found)?;
        } else if path.extension().is_some_and(|extension| extension == "tql") {
            let relative = path
                .strip_prefix(repo_path)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            found.push(relative);
        }
    }
    Ok(())
}

fn normalize_relative(path: &str) -> String {
    let trimmed = path.trim().trim_start_matches('/');
    trimmed.strip_prefix("./").unwrap_or(trimmed).to_string()
}

fn validate_rendered_doc(
    repo_path: &Path,
    package: &Value,
    questions_path: &str,
    file: &QuestionsFile,
) -> Result<()> {
    let doc_path = repo_path.join(DOC_PATH);
    if !doc_path.is_file() {
        return Ok(());
    }
    let current = std::fs::read_to_string(&doc_path)
        .map_err(|error| invalid(format!("failed to read {DOC_PATH}: {error}")))?;
    if current != render_questions_doc(package, questions_path, file) {
        return Err(invalid(format!(
            "{DOC_PATH} differs from what {questions_path} renders; run \
             `ont check-questions --repo <repo> --write-generated` to regenerate it"
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Renders the questions document: a title, one `##` section per group in
/// order of first appearance, and the group's questions as a numbered list
/// carrying each question's own number.
pub fn render_questions_doc(package: &Value, questions_path: &str, file: &QuestionsFile) -> String {
    let name = package
        .get("displayName")
        .and_then(Value::as_str)
        .or_else(|| package.get("name").and_then(Value::as_str))
        .unwrap_or("this package");
    let mut groups: Vec<(&str, Vec<&Question>)> = Vec::new();
    for question in &file.questions {
        match groups
            .iter_mut()
            .find(|(group, _)| *group == question.group)
        {
            Some((_, members)) => members.push(question),
            None => groups.push((question.group.as_str(), vec![question])),
        }
    }

    let mut out = String::new();
    out.push_str(&format!("# Competency questions: {name}\n\n"));
    out.push_str(&format!(
        "<!-- Generated from {questions_path} by `ont check-questions --write-generated`. Do not edit by hand. -->\n"
    ));
    for (group, members) in groups {
        out.push_str(&format!("\n## {group}\n\n"));
        for question in members {
            out.push_str(&format!("{}. {}\n", question.number, question.text));
        }
    }
    out
}

/// Writes the rendered document to [`DOC_PATH`], returning its path.
pub fn write_questions_doc(
    repo_path: &Path,
    package: &Value,
    questions_path: &str,
    file: &QuestionsFile,
) -> Result<PathBuf> {
    let doc_path = repo_path.join(DOC_PATH);
    if let Some(parent) = doc_path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| Error::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    std::fs::write(
        &doc_path,
        render_questions_doc(package, questions_path, file),
    )
    .map_err(|source| Error::Io {
        path: doc_path.clone(),
        source,
    })?;
    Ok(doc_path)
}

// ---------------------------------------------------------------------------
// Digest, last run, release gate
// ---------------------------------------------------------------------------

/// SHA-256 over everything a question's answer depends on: the questions file,
/// every fixture, every query file, and the package's own schema and data files.
/// Order: the questions file, then fixtures in declared order, then query files
/// sorted by path (each once), then `package.json` `schemas[].file` in listed
/// order, then `package.json` `data` in listed order. Each file contributes
/// `path NUL contents NUL`. A change to any of them makes the last run stale.
pub fn questions_digest(
    repo_path: &Path,
    package: &Value,
    questions_path: &str,
    file: &QuestionsFile,
) -> Result<String> {
    let mut paths = vec![questions_path.to_string()];
    paths.extend(file.fixtures.iter().cloned());
    let queries: BTreeSet<&str> = file
        .questions
        .iter()
        .map(|question| question.query.as_str())
        .collect();
    paths.extend(queries.into_iter().map(str::to_string));
    paths.extend(
        package
            .get("schemas")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|schema| schema.get("file").and_then(Value::as_str))
            .map(str::to_string),
    );
    paths.extend(
        package
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string),
    );

    let mut context = ring::digest::Context::new(&ring::digest::SHA256);
    for path in paths {
        let full_path = joined(repo_path, &path);
        let bytes = std::fs::read(&full_path).map_err(|source| Error::Io {
            path: full_path.clone(),
            source,
        })?;
        context.update(path.as_bytes());
        context.update(&[0]);
        context.update(&bytes);
        context.update(&[0]);
    }
    Ok(context
        .finish()
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// The record written to [`LAST_RUN_PATH`] after every question passed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LastRun {
    pub package: String,
    pub version: String,
    pub ran_at: String,
    pub questions_digest: String,
    pub results: Vec<LastRunResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LastRunResult {
    pub id: String,
    pub number: u64,
    pub rows: usize,
}

/// Refuses a release unless the questions were run (successfully) after their
/// last change. Waived packages pass.
pub fn check_release_gate(repo_path: &Path, package: &Value) -> Result<()> {
    let CompetencyDeclaration::Declared { path, file } = read_declaration(repo_path, package)?
    else {
        return Ok(());
    };
    let run_hint = "run `ont check-questions --repo <repo>` and commit competency/last-run.json";
    let last_run_path = repo_path.join(LAST_RUN_PATH);
    if !last_run_path.is_file() {
        return Err(invalid(format!(
            "{LAST_RUN_PATH} not found: the competency questions have never been run; {run_hint}"
        )));
    }
    let text = std::fs::read_to_string(&last_run_path)
        .map_err(|error| invalid(format!("failed to read {LAST_RUN_PATH}: {error}")))?;
    let last_run: LastRun = serde_json::from_str(&text)
        .map_err(|error| invalid(format!("invalid {LAST_RUN_PATH}: {error}; {run_hint}")))?;
    let digest = questions_digest(repo_path, package, &path, &file)?;
    if last_run.questions_digest != digest {
        return Err(invalid(format!(
            "{LAST_RUN_PATH} is stale: the questions, queries, fixtures, schema or data changed \
             after the last run; {run_hint}"
        )));
    }
    Ok(())
}

/// Current UTC time as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn iso_now() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    iso_from_unix(seconds)
}

fn iso_from_unix(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let remainder = seconds % 86_400;
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        remainder / 3_600,
        (remainder % 3_600) / 60,
        remainder % 60
    )
}

// ---------------------------------------------------------------------------
// Run
// ---------------------------------------------------------------------------

/// The outcome of one question's query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionOutcome {
    pub id: String,
    pub number: u64,
    pub rows: usize,
    pub error: Option<String>,
}

impl QuestionOutcome {
    pub fn passed(&self) -> bool {
        self.error.is_none() && self.rows > 0
    }
}

/// One package in dependency order, ready to load.
#[derive(Debug, Clone)]
pub struct OrderedPackage {
    pub name: String,
    pub root: PathBuf,
    pub manifest: Value,
}

/// Resolves the package's dependencies (recursively) to sibling repositories
/// named `ontology-<name>` next to `repo_path`, returning every package in load
/// order (dependencies first, the package itself last). Mirrors the package
/// TypeDB bootstrap test (`tools/package_contract/validate_typedb_bootstrap.mjs`).
pub fn resolve_package_order(repo_path: &Path) -> Result<Vec<OrderedPackage>> {
    let workspace_root = repo_path
        .parent()
        .ok_or_else(|| invalid(format!("{} has no parent directory", repo_path.display())))?
        .to_path_buf();
    let mut ordered = Vec::new();
    let mut seen = BTreeSet::new();
    let mut visiting = BTreeSet::new();
    visit_package(
        repo_path,
        &workspace_root,
        &mut seen,
        &mut visiting,
        &mut ordered,
    )?;
    Ok(ordered)
}

fn visit_package(
    package_root: &Path,
    workspace_root: &Path,
    seen: &mut BTreeSet<String>,
    visiting: &mut BTreeSet<String>,
    ordered: &mut Vec<OrderedPackage>,
) -> Result<()> {
    let manifest = read_package_json(package_root)?;
    let name = manifest
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if seen.contains(&name) {
        return Ok(());
    }
    if !visiting.insert(name.clone()) {
        return Err(invalid(format!(
            "cyclic ontology dependency detected at {name}"
        )));
    }
    for dependency in manifest
        .get("dependencies")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(dependency_name) = dependency.get("name").and_then(Value::as_str) else {
            continue;
        };
        let dependency_root = workspace_root.join(format!("ontology-{dependency_name}"));
        if !dependency_root.join("package.json").is_file() {
            return Err(invalid(format!(
                "dependency '{dependency_name}' of {name} not found at {}",
                dependency_root.display()
            )));
        }
        visit_package(&dependency_root, workspace_root, seen, visiting, ordered)?;
    }
    visiting.remove(&name);
    if seen.insert(name.clone()) {
        ordered.push(OrderedPackage {
            name,
            root: package_root.to_path_buf(),
            manifest,
        });
    }
    Ok(())
}

fn read_package_json(package_root: &Path) -> Result<Value> {
    let path = package_root.join("package.json");
    let text = std::fs::read_to_string(&path).map_err(|source| Error::Io {
        path: path.clone(),
        source,
    })?;
    serde_json::from_str(&text).map_err(|source| Error::Json { path, source })
}

/// A progress callback: called with a line of text as loading proceeds.
pub type Progress<'a> = &'a mut dyn FnMut(&str);

/// Loads the package and its dependencies into `database` on `target`, then the
/// fixtures, then runs every question's query in a read transaction.
///
/// Loading: every package's schema assets (its `schemas[].file` entries, in
/// `assembly.loadOrder`) go in one schema transaction validated at commit,
/// dependencies first; then every other `.tql` load-order asset is applied as a
/// write, in order; then the fixtures, in order. A load failure is an error; a
/// query failure is recorded on that question's outcome.
pub async fn run_competency_questions(
    target: &TypeDbTarget,
    database: &str,
    repo_path: &Path,
    file: &QuestionsFile,
    progress: Progress<'_>,
) -> Result<RunOutcome> {
    let ordered = resolve_package_order(repo_path)?;
    progress(&format!(
        "package order: {}",
        ordered
            .iter()
            .map(|package| package.name.as_str())
            .collect::<Vec<_>>()
            .join(" -> ")
    ));

    let mut schema_blobs = Vec::new();
    let mut writes: Vec<(String, PathBuf)> = Vec::new();
    for package in &ordered {
        let schema_files: BTreeSet<&str> = package
            .manifest
            .get("schemas")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|schema| schema.get("file").and_then(Value::as_str))
            .collect();
        for asset in package
            .manifest
            .get("assembly")
            .and_then(|assembly| assembly.get("loadOrder"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !asset.ends_with(".tql") {
                continue;
            }
            let path = joined(&package.root, asset);
            if schema_files.contains(asset) {
                schema_blobs.push(read_text(&path)?);
            } else {
                writes.push((format!("{}:{asset}", package.name), path));
            }
        }
    }
    for fixture in &file.fixtures {
        writes.push((format!("fixture:{fixture}"), joined(repo_path, fixture)));
    }

    create_database(target, database).await?;
    progress(&format!("loading {} schema files", schema_blobs.len()));
    apply_schema(target, database, &schema_blobs)
        .await
        .map_err(|error| invalid(format!("schema load failed: {error}")))?;
    progress(&format!("loading {} data and fixture files", writes.len()));
    for (label, path) in &writes {
        let text = read_text(path)?;
        if text.trim().is_empty() {
            continue;
        }
        apply_write(target, database, &text)
            .await
            .map_err(|error| invalid(format!("load failed for {label}: {error}")))?;
    }

    let driver = connect(target).await?;
    let mut outcomes = Vec::new();
    for question in &file.questions {
        let text = read_text(&joined(repo_path, &question.query))?;
        let outcome = match count_rows(&driver, database, &text).await {
            Ok(rows) => QuestionOutcome {
                id: question.id.clone(),
                number: question.number,
                rows,
                error: None,
            },
            Err(error) => QuestionOutcome {
                id: question.id.clone(),
                number: question.number,
                rows: 0,
                error: Some(error),
            },
        };
        outcomes.push(outcome);
    }
    let package_name = ordered
        .last()
        .map(|package| package.name.clone())
        .unwrap_or_default();
    let installed = check_installed_questions(&driver, database, &package_name, file).await;
    Ok(RunOutcome {
        outcomes,
        installed,
    })
}

/// The result of a run: each question's outcome, and whether the installed
/// `StoredQuestion` records match the questions file (`Ok(count)` or the
/// problem).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutcome {
    pub outcomes: Vec<QuestionOutcome>,
    pub installed: std::result::Result<usize, String>,
}

impl RunOutcome {
    pub fn passed(&self) -> bool {
        self.installed.is_ok() && self.outcomes.iter().all(QuestionOutcome::passed)
    }
}

/// Confirms that the package installed exactly one `StoredQuestion` per
/// question and that each is answered by a `StoredQuery` with query text.
async fn check_installed_questions(
    driver: &TypeDBDriver,
    database: &str,
    package_name: &str,
    file: &QuestionsFile,
) -> std::result::Result<usize, String> {
    let package = crate::stored_questions::literal(package_name);
    let installed = column_values(
        driver,
        database,
        &format!(
            "match $q isa StoredQuestion, has storedPackage {package}, has storedQuestionId $id; select $id;"
        ),
    )
    .await?;
    let answered = column_values(
        driver,
        database,
        &format!(
            "match $q isa StoredQuestion, has storedPackage {package}, has storedQuestionId $id; \
             storedAnswer (question: $q, query: $s); $s isa StoredQuery, has storedQueryText $t; select $id;"
        ),
    )
    .await?;
    let expected = file.questions.len();
    if installed.len() != expected {
        return Err(format!(
            "{} StoredQuestion records installed for {package_name}, expected {expected}",
            installed.len()
        ));
    }
    let unanswered: Vec<&String> = installed.difference(&answered).collect();
    if !unanswered.is_empty() {
        return Err(format!(
            "installed questions without an answer query: {}",
            unanswered
                .iter()
                .map(|id| id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok(installed.len())
}

/// The distinct string values of the `id` column.
async fn column_values(
    driver: &TypeDBDriver,
    database: &str,
    query: &str,
) -> std::result::Result<BTreeSet<String>, String> {
    let transaction = driver
        .transaction(database, TransactionType::Read)
        .await
        .map_err(|error| error.to_string())?;
    let answer = transaction
        .query(query)
        .await
        .map_err(|error| error.to_string())?;
    let mut values = BTreeSet::new();
    if let QueryAnswer::ConceptRowStream(_, mut stream) = answer {
        while let Some(row) = stream.next().await {
            let row = row.map_err(|error| error.to_string())?;
            if let Some(value) = row
                .get("id")
                .map_err(|error| error.to_string())?
                .and_then(|concept| concept.try_get_string())
            {
                values.insert(value.to_string());
            }
        }
    }
    Ok(values)
}

async fn connect(target: &TypeDbTarget) -> Result<TypeDBDriver> {
    let addresses = Addresses::try_from_address_str(&target.address)
        .map_err(|error| Error::TypeDb(error.to_string()))?;
    let credentials = Credentials::new(&target.username, &target.password);
    TypeDBDriver::new(
        addresses,
        credentials,
        DriverOptions::new(DriverTlsConfig::disabled()),
    )
    .await
    .map_err(|error| Error::TypeDb(error.to_string()))
}

async fn count_rows(
    driver: &TypeDBDriver,
    database: &str,
    query: &str,
) -> std::result::Result<usize, String> {
    let transaction = driver
        .transaction(database, TransactionType::Read)
        .await
        .map_err(|error| error.to_string())?;
    let answer = transaction
        .query(query)
        .await
        .map_err(|error| error.to_string())?;
    let mut rows = 0;
    match answer {
        QueryAnswer::Ok(_) => {
            return Err("the query returned no rows stream (not a read query)".to_string())
        }
        QueryAnswer::ConceptRowStream(_, mut stream) => {
            while let Some(row) = stream.next().await {
                row.map_err(|error| error.to_string())?;
                rows += 1;
            }
        }
        QueryAnswer::ConceptDocumentStream(_, mut stream) => {
            while let Some(document) = stream.next().await {
                document.map_err(|error| error.to_string())?;
                rows += 1;
            }
        }
    }
    Ok(rows)
}

fn read_text(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn joined(repo_path: &Path, relative_path: &str) -> PathBuf {
    repo_path.join(relative_path.trim_start_matches('/'))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const QUERY: &str = "# 7. Which distinctions?\nmatch $d isa thing, has name $n; select $n;\n";

    fn write(root: &Path, relative_path: &str, contents: &str) {
        let path = root.join(relative_path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    fn questions_json() -> Value {
        json!({
            "formatVersion": 1,
            "fixtures": ["fixtures/sample.tql"],
            "questions": [
                {"id": "q07", "number": 7, "group": "What each source says",
                 "text": "Which distinctions does a source draw?", "query": "queries/q07.tql"},
                {"id": "q01", "number": 1, "group": "What was read",
                 "text": "Which sources have been read?", "query": "queries/q01.tql"},
                {"id": "q08", "number": 8, "group": "What each source says",
                 "text": "Into what kinds does a source divide a term?", "query": "queries/q08.tql"}
            ]
        })
    }

    fn package() -> Value {
        json!({
            "name": "custom",
            "displayName": "Custom Knowledge",
            "version": "1.0.0",
            "schemas": [
                {"name": "stored-queries", "file": "schema/stored-queries.tql"},
                {"name": "custom", "file": "schema/custom.tql"}
            ],
            "data": ["data/seed.tql", "data/custom-questions.tql"],
            "queries": [],
            "competencyQuestions": "competency/questions.json"
        })
    }

    /// A fixture repository with a valid questions file; `questions` replaces
    /// the questions JSON when given.
    fn fixture(questions: Option<&Value>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "fixtures/sample.tql", "insert $x isa thing;\n");
        write(root, "schema/custom.tql", "define\nentity thing;\n");
        write(root, "data/seed.tql", "insert $x isa thing;\n");
        write(root, "queries/q07.tql", QUERY);
        write(
            root,
            "queries/q01.tql",
            "match $x isa thing; fetch { \"x\": $x.* };\n",
        );
        write(root, "queries/q08.tql", QUERY);
        let questions = questions.cloned().unwrap_or_else(questions_json);
        write(
            root,
            "competency/questions.json",
            &serde_json::to_string_pretty(&questions).unwrap(),
        );
        // The installed form, when the questions file is valid enough to read.
        if let Ok(CompetencyDeclaration::Declared { path, file }) =
            read_declaration(root, &package())
        {
            crate::stored_questions::write_generated(root, &package(), &path, &file).unwrap();
        }
        dir
    }

    fn assert_message(dir: &tempfile::TempDir, package: &Value, expected: &str) {
        match validate_competency_questions(dir.path(), package) {
            Err(Error::Version(message)) => assert_eq!(message, expected),
            other => panic!("expected Version({expected:?}), got {other:?}"),
        }
    }

    fn with_question_field(field: &str, value: Value) -> Value {
        let mut questions = questions_json();
        questions["questions"][0][field] = value;
        questions
    }

    #[test]
    fn accepts_a_valid_questions_file() {
        let dir = fixture(None);
        let declaration = validate_competency_questions(dir.path(), &package()).unwrap();
        let CompetencyDeclaration::Declared { path, file } = declaration else {
            panic!("expected declared questions");
        };
        assert_eq!(path, "competency/questions.json");
        assert_eq!(file.fixtures, vec!["fixtures/sample.tql"]);
        assert_eq!(file.questions.len(), 3);
        assert_eq!(file.questions[0].id, "q07");
    }

    #[test]
    fn missing_field_names_both_ways_to_satisfy_it() {
        let dir = fixture(None);
        let mut package = package();
        package.as_object_mut().unwrap().remove(FIELD);
        assert_message(
            &dir,
            &package,
            "package.json 'competencyQuestions' is required: set it to the questions file path \
             (\"competency/questions.json\") or to {\"waived\": \"<reason>\"} for a package that \
             deliberately has none",
        );
    }

    #[test]
    fn accepts_a_waiver_with_a_reason() {
        let dir = tempfile::tempdir().unwrap();
        let mut package = package();
        package[FIELD] = json!({"waived": "upper ontology; questions live in dependents"});
        assert_eq!(
            validate_competency_questions(dir.path(), &package).unwrap(),
            CompetencyDeclaration::Waived {
                reason: "upper ontology; questions live in dependents".to_string()
            }
        );
    }

    #[test]
    fn rejects_an_empty_waiver() {
        let dir = tempfile::tempdir().unwrap();
        let mut package = package();
        package[FIELD] = json!({"waived": "  "});
        assert_message(
            &dir,
            &package,
            "package.json 'competencyQuestions' waiver must be {\"waived\": \"<non-empty reason>\"}",
        );
    }

    #[test]
    fn rejects_a_missing_questions_file() {
        let dir = tempfile::tempdir().unwrap();
        assert_message(
            &dir,
            &package(),
            "competency questions file not found: competency/questions.json",
        );
    }

    #[test]
    fn rejects_a_wrong_format_version() {
        let mut questions = questions_json();
        questions["formatVersion"] = json!(2);
        let dir = fixture(Some(&questions));
        assert_message(
            &dir,
            &package(),
            "competency/questions.json: 'formatVersion' must be 1",
        );
    }

    #[test]
    fn rejects_a_missing_fixture() {
        let mut questions = questions_json();
        questions["fixtures"] = json!(["fixtures/absent.tql"]);
        let dir = fixture(Some(&questions));
        assert_message(
            &dir,
            &package(),
            "competency/questions.json: fixture not found: fixtures/absent.tql",
        );
    }

    #[test]
    fn rejects_an_empty_question_list() {
        let mut questions = questions_json();
        questions["questions"] = json!([]);
        let dir = fixture(Some(&questions));
        assert_message(
            &dir,
            &package(),
            "competency/questions.json: 'questions' must contain at least one question",
        );
    }

    #[test]
    fn rejects_a_duplicate_id() {
        let dir = fixture(Some(&with_question_field("id", json!("q01"))));
        assert_message(
            &dir,
            &package(),
            "competency/questions.json: duplicate question id 'q01'",
        );
    }

    #[test]
    fn rejects_a_reused_number() {
        let dir = fixture(Some(&with_question_field("number", json!(1))));
        assert_message(
            &dir,
            &package(),
            "competency/questions.json: question 'q01' reuses number 1 (already used by 'q07')",
        );
    }

    #[test]
    fn rejects_a_non_positive_number() {
        let dir = fixture(Some(&with_question_field("number", json!(0))));
        assert_message(
            &dir,
            &package(),
            "competency/questions.json: question 'q07' 'number' must be a positive integer",
        );
    }

    #[test]
    fn rejects_empty_text_and_group() {
        let dir = fixture(Some(&with_question_field("text", json!(""))));
        assert_message(
            &dir,
            &package(),
            "competency/questions.json: question 'q07' 'text' must be a non-empty string",
        );
        let dir = fixture(Some(&with_question_field("group", json!(null))));
        assert_message(
            &dir,
            &package(),
            "competency/questions.json: question 'q07' 'group' must be a non-empty string",
        );
    }

    #[test]
    fn rejects_a_missing_query_file() {
        let dir = fixture(Some(&with_question_field(
            "query",
            json!("queries/absent.tql"),
        )));
        assert_message(
            &dir,
            &package(),
            "competency/questions.json: question 'q07' query file not found: queries/absent.tql",
        );
    }

    #[test]
    fn rejects_a_write_query() {
        let dir = fixture(None);
        write(
            dir.path(),
            "queries/q07.tql",
            "match $x isa thing; delete $x;\n",
        );
        assert_message(
            &dir,
            &package(),
            "competency/questions.json: question 'q07' query queries/q07.tql must hold exactly \
             one read query: it holds a write or schema stage ('delete')",
        );
    }

    #[test]
    fn rejects_several_queries_in_one_file() {
        let dir = fixture(None);
        write(
            dir.path(),
            "queries/q07.tql",
            "match $x isa thing;\nend;\nmatch $y isa thing;\nend;\n",
        );
        assert_message(
            &dir,
            &package(),
            "competency/questions.json: question 'q07' query queries/q07.tql must hold exactly \
             one read query: it holds more than one query ('end;')",
        );
    }

    #[test]
    fn rejects_an_orphan_query_file() {
        let dir = fixture(None);
        write(dir.path(), "queries/nested/q99.tql", QUERY);
        assert_message(
            &dir,
            &package(),
            "queries/nested/q99.tql is not used by any competency question and is not listed in \
             package.json 'queries'",
        );
    }

    #[test]
    fn accepts_a_query_listed_in_package_queries() {
        let dir = fixture(None);
        write(dir.path(), "queries/axioms.tql", QUERY);
        let mut package = package();
        package["queries"] = json!(["queries/axioms.tql"]);
        validate_competency_questions(dir.path(), &package).unwrap();
    }

    #[test]
    fn read_query_check_ignores_keywords_in_strings_comments_and_names() {
        assert_eq!(
            check_single_read_query(
                "# insert nothing\nmatch $x isa update-record, has name \"delete me\"; $put isa thing;\n"
            ),
            Ok(())
        );
        assert_eq!(
            check_single_read_query("# only a comment\n"),
            Err("it is empty".to_string())
        );
        assert_eq!(
            check_single_read_query("insert $x isa thing;"),
            Err("it starts with 'insert', not 'match', 'with' or 'fetch'".to_string())
        );
    }

    #[test]
    fn renders_groups_in_order_of_first_appearance() {
        let dir = fixture(None);
        let CompetencyDeclaration::Declared { path, file } =
            read_declaration(dir.path(), &package()).unwrap()
        else {
            panic!("expected declared questions");
        };
        assert_eq!(
            render_questions_doc(&package(), &path, &file),
            "# Competency questions: Custom Knowledge\n\n\
             <!-- Generated from competency/questions.json by `ont check-questions --write-generated`. Do not edit by hand. -->\n\
             \n## What each source says\n\n\
             7. Which distinctions does a source draw?\n\
             8. Into what kinds does a source divide a term?\n\
             \n## What was read\n\n\
             1. Which sources have been read?\n"
        );
    }

    #[test]
    fn rejects_a_stale_rendered_doc_and_accepts_a_fresh_one() {
        let dir = fixture(None);
        write(dir.path(), DOC_PATH, "# hand edited\n");
        assert_message(
            &dir,
            &package(),
            "docs/competency-questions.md differs from what competency/questions.json renders; \
             run `ont check-questions --repo <repo> --write-generated` to regenerate it",
        );
        let CompetencyDeclaration::Declared { path, file } =
            read_declaration(dir.path(), &package()).unwrap()
        else {
            panic!("expected declared questions");
        };
        write_questions_doc(dir.path(), &package(), &path, &file).unwrap();
        validate_competency_questions(dir.path(), &package()).unwrap();
    }

    #[test]
    fn release_gate_requires_a_matching_last_run() {
        let dir = fixture(None);
        assert_eq!(
            check_release_gate(dir.path(), &package())
                .unwrap_err()
                .to_string(),
            "version error: competency/last-run.json not found: the competency questions have \
             never been run; run `ont check-questions --repo <repo>` and commit \
             competency/last-run.json"
        );

        let CompetencyDeclaration::Declared { path, file } =
            read_declaration(dir.path(), &package()).unwrap()
        else {
            panic!("expected declared questions");
        };
        let last_run = LastRun {
            package: "custom".to_string(),
            version: "1.0.0".to_string(),
            ran_at: iso_now(),
            questions_digest: questions_digest(dir.path(), &package(), &path, &file).unwrap(),
            results: Vec::new(),
        };
        write(
            dir.path(),
            LAST_RUN_PATH,
            &serde_json::to_string_pretty(&last_run).unwrap(),
        );
        check_release_gate(dir.path(), &package()).unwrap();

        write(dir.path(), "queries/q08.tql", "match $y isa thing;\n");
        assert_eq!(
            check_release_gate(dir.path(), &package())
                .unwrap_err()
                .to_string(),
            "version error: competency/last-run.json is stale: the questions, queries, fixtures, \
             schema or data changed after the last run; run `ont check-questions --repo <repo>` \
             and commit competency/last-run.json"
        );
    }

    fn digest_of(dir: &tempfile::TempDir) -> String {
        let CompetencyDeclaration::Declared { path, file } =
            read_declaration(dir.path(), &package()).unwrap()
        else {
            panic!("expected declared questions");
        };
        questions_digest(dir.path(), &package(), &path, &file).unwrap()
    }

    #[test]
    fn digest_covers_questions_queries_fixtures_schema_and_data() {
        let changes: &[(&str, &str)] = &[
            (
                "competency/questions.json",
                "{\"formatVersion\": 1, \"questions\": []}",
            ),
            ("queries/q01.tql", "match $y isa thing;\n"),
            ("fixtures/sample.tql", "insert $y isa thing;\n"),
            ("schema/custom.tql", "define\nentity other;\n"),
            ("data/seed.tql", "insert $y isa thing;\n"),
        ];
        for (relative_path, contents) in changes {
            let dir = fixture(None);
            let before = digest_of(&dir);
            assert_eq!(before, digest_of(&dir), "digest must be stable");
            if *relative_path == "competency/questions.json" {
                // Keep the file valid: change only a question's text.
                let mut questions = questions_json();
                questions["questions"][0]["text"] = json!("Changed?");
                write(
                    dir.path(),
                    relative_path,
                    &serde_json::to_string_pretty(&questions).unwrap(),
                );
            } else {
                write(dir.path(), relative_path, contents);
            }
            assert_ne!(
                before,
                digest_of(&dir),
                "changing {relative_path} must change the digest"
            );
        }
    }

    #[test]
    fn digest_ignores_files_the_questions_do_not_depend_on() {
        let dir = fixture(None);
        let before = digest_of(&dir);
        write(dir.path(), "README.md", "# notes\n");
        write(dir.path(), "manifests/resources.json", "{}");
        assert_eq!(before, digest_of(&dir));
    }

    #[test]
    fn release_gate_skips_waived_packages() {
        let dir = tempfile::tempdir().unwrap();
        let mut package = package();
        package[FIELD] = json!({"waived": "no questions"});
        check_release_gate(dir.path(), &package).unwrap();
    }

    #[test]
    fn iso_time_formats_known_instants() {
        assert_eq!(iso_from_unix(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_from_unix(1_795_132_800), "2026-11-20T00:00:00Z");
    }
}
