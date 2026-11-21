//! The installed form of a package's competency questions.
//!
//! The app (Lingo) shows a package's questions only when they are installed in
//! the database as `StoredQuestion` / `StoredQuery` records joined by
//! `storedAnswer`. This module generates, from `competency/questions.json`:
//!
//! - `schema/stored-queries.tql`: the exact schema text every question-carrying
//!   package ships (OneApp's `StoredQuerySchema.typeQL`);
//! - `data/<package>-questions.tql`: one `StoredQuery` and one `StoredQuestion`
//!   per question, in the shape OneApp's `StoredQueryStore.dataTypeQL` writes and
//!   its reads expect (the same stages and attributes; one paragraph per
//!   question, so the file splits into apply units only between questions).
//!
//! Key rules (the app's): question ids are `<package>/q-NN` (the question
//! number, two digits); query names are `<package>/<slug of the title>`, unique
//! with a numeric suffix. Each record is matched on its key with `put` and its
//! other attributes are set with `update`, so installing twice, or over an older
//! copy, leaves one record per key with the latest text. `storedVersion` is the
//! package version at which the question's entry last changed: regenerating
//! keeps the version already in the file for an unchanged entry and stamps the
//! current package version on a new or changed one.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;

use crate::competency_questions::QuestionsFile;
use crate::error::{Error, Result};
use crate::executable_package::APPLY_UNITS_ROOT;

/// The schema name in `package.json` `schemas`.
pub const SCHEMA_NAME: &str = "stored-queries";
/// The schema file's package-relative path.
pub const SCHEMA_PATH: &str = "schema/stored-queries.tql";
/// OneApp's `StoredQuerySchema.typeQL`, character for character.
pub const SCHEMA_TEXT: &str = "define
  attribute storedPackage, value string;
  attribute storedVersion, value string;
  attribute storedQuestionId, value string;
  attribute storedQuestionText, value string;
  attribute storedQuestionOrder, value integer;
  attribute storedQueryName, value string;
  attribute storedQueryTitle, value string;
  attribute storedQueryText, value string;
  attribute storedQueryExplanation, value string;
  entity StoredQuestion,
    owns storedQuestionId @key,
    owns storedQuestionText,
    owns storedPackage,
    owns storedVersion,
    owns storedQuestionOrder @card(0..1),
    plays storedAnswer:question;
  entity StoredQuery,
    owns storedQueryName @key,
    owns storedQueryTitle,
    owns storedQueryText,
    owns storedPackage,
    owns storedVersion,
    owns storedQueryExplanation @card(0..1),
    plays storedAnswer:query;
  relation storedAnswer,
    relates question,
    relates query;
";

const GENERATE_HINT: &str = "run `ont check-questions --repo <repo> --write-generated`";

fn invalid<S: Into<String>>(message: S) -> Error {
    Error::Version(message.into())
}

/// `data/<package>-questions.tql`.
pub fn data_path(package_name: &str) -> String {
    format!("data/{package_name}-questions.tql")
}

/// One question in its installed form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRecord {
    /// `<package>/q-NN`.
    pub question_id: String,
    pub order: u64,
    pub question_text: String,
    /// `<package>/<slug>`.
    pub query_name: String,
    pub title: String,
    /// The query file without its leading comment lines, trimmed.
    pub query_text: String,
    pub explanation: Option<String>,
}

/// The app's slug: lower-case ASCII letters and digits, words joined by `-`,
/// at most 60 characters (whole words only); `fallback` when nothing is left.
pub fn slug(text: &str, fallback: &str) -> String {
    let lowered = text.to_lowercase();
    let mut slug = String::new();
    for word in lowered
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
    {
        let next = if slug.is_empty() {
            word.to_string()
        } else {
            format!("{slug}-{word}")
        };
        if next.chars().count() > 60 {
            break;
        }
        slug = next;
    }
    if slug.is_empty() {
        fallback.to_string()
    } else {
        slug
    }
}

/// `value` as a TypeQL string literal, escaped exactly as the app escapes it
/// (backslash, quote, newline, carriage return, tab).
pub fn literal(value: &str) -> String {
    let mut result = String::from("\"");
    for character in value.chars() {
        match character {
            '\\' => result.push_str("\\\\"),
            '"' => result.push_str("\\\""),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            other => result.push(other),
        }
    }
    result.push('"');
    result
}

/// A query file's text without its leading `#` comment (and blank) lines,
/// trimmed.
pub fn query_body(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines
        .iter()
        .position(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with('#')
        })
        .unwrap_or(lines.len());
    lines[start..].join("\n").trim().to_string()
}

/// The installed records for every question, in questions-file order.
pub fn stored_records(
    repo_path: &Path,
    package_name: &str,
    file: &QuestionsFile,
) -> Result<Vec<StoredRecord>> {
    let mut used_names = BTreeSet::new();
    let mut records = Vec::new();
    for question in &file.questions {
        let title = question
            .title
            .clone()
            .unwrap_or_else(|| question.text.clone());
        let base = format!(
            "{package_name}/{}",
            slug(&title, &format!("q-{}", question.number))
        );
        let mut name = base.clone();
        let mut suffix = 2;
        while !used_names.insert(name.clone()) {
            name = format!("{base}-{suffix}");
            suffix += 1;
        }
        let query_path = repo_path.join(question.query.trim_start_matches('/'));
        let text = std::fs::read_to_string(&query_path).map_err(|source| Error::Io {
            path: query_path.clone(),
            source,
        })?;
        records.push(StoredRecord {
            question_id: format!("{package_name}/q-{:02}", question.number),
            order: question.number,
            question_text: question.text.clone(),
            query_name: name,
            title,
            query_text: query_body(&text),
            explanation: question.explanation.clone(),
        });
    }
    Ok(records)
}

fn render_paragraph(
    index: usize,
    package_name: &str,
    version: &str,
    record: &StoredRecord,
) -> String {
    let query = format!("$query{index}");
    let question = format!("$question{index}");
    let mut lines = vec![
        format!(
            "put {query} isa StoredQuery, has storedQueryName {};",
            literal(&record.query_name)
        ),
        // `update` shares its line with the first statement: a bare `update`
        // line would make the file look put-only to `prepare-package`, which
        // would then cut a question's records apart into separate units.
        format!(
            "update {query} has storedQueryTitle {};",
            literal(&record.title)
        ),
        format!(
            "  {query} has storedQueryText {};",
            literal(&record.query_text)
        ),
        format!("  {query} has storedPackage {};", literal(package_name)),
        format!("  {query} has storedVersion {};", literal(version)),
    ];
    if let Some(explanation) = &record.explanation {
        lines.push(format!(
            "  {query} has storedQueryExplanation {};",
            literal(explanation)
        ));
    }
    lines.extend([
        format!(
            "put {question} isa StoredQuestion, has storedQuestionId {};",
            literal(&record.question_id)
        ),
        format!(
            "update {question} has storedQuestionText {};",
            literal(&record.question_text)
        ),
        format!("  {question} has storedPackage {};", literal(package_name)),
        format!("  {question} has storedVersion {};", literal(version)),
        format!("  {question} has storedQuestionOrder {};", record.order),
        format!("put storedAnswer (question: {question}, query: {query});"),
    ]);
    lines.join("\n")
}

fn version_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r#"has storedVersion "((?:[^"\\]|\\.)*)""#).expect("version regex is valid")
    })
}

fn variable_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"\$(query|question)\d+").expect("variable regex is valid"))
}

fn question_id_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r#"has storedQuestionId "((?:[^"\\]|\\.)*)""#).expect("id regex is valid")
    })
}

/// A paragraph with its versions blanked and variable numbers removed, so two
/// renderings of the same entry compare equal whatever their version or place.
fn comparable(paragraph: &str) -> String {
    let without_versions = version_regex().replace_all(paragraph.trim(), "has storedVersion \"\"");
    variable_regex()
        .replace_all(&without_versions, "$$$1")
        .into_owned()
}

/// The version and comparable text of each entry in an existing generated
/// file, keyed by question id.
fn existing_entries(text: &str) -> HashMap<String, (String, String)> {
    let mut entries = HashMap::new();
    let blank_line = Regex::new(r"\n\s*\n").expect("paragraph regex is valid");
    for paragraph in blank_line.split(text) {
        let paragraph: String = paragraph
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n");
        let Some(id) = question_id_regex()
            .captures(&paragraph)
            .map(|captures| captures[1].to_string())
        else {
            continue;
        };
        let Some(version) = version_regex()
            .captures(&paragraph)
            .map(|captures| captures[1].to_string())
        else {
            continue;
        };
        entries.insert(id, (version, comparable(&paragraph)));
    }
    entries
}

/// Renders the questions data file. `existing` is the current file, if any:
/// an entry whose text is unchanged keeps its version; a new or changed entry
/// gets `version`.
pub fn render_data(
    package_name: &str,
    version: &str,
    questions_path: &str,
    records: &[StoredRecord],
    existing: Option<&str>,
) -> String {
    let previous = existing.map(existing_entries).unwrap_or_default();
    let mut out = format!(
        "# Generated from {questions_path} by `ont check-questions --write-generated`. Do not edit by hand.\n\
         # One StoredQuery and one StoredQuestion per competency question, joined by storedAnswer.\n"
    );
    for (index, record) in records.iter().enumerate() {
        let fresh = render_paragraph(index + 1, package_name, version, record);
        let paragraph = match previous.get(&record.question_id) {
            Some((kept, before)) if *before == comparable(&fresh) => {
                render_paragraph(index + 1, package_name, kept, record)
            }
            _ => fresh,
        };
        out.push('\n');
        out.push_str(&paragraph);
        out.push('\n');
    }
    out
}

/// The generated files, `(path, contents)`: the schema, then the data.
pub fn generated_files(
    repo_path: &Path,
    package: &Value,
    questions_path: &str,
    file: &QuestionsFile,
) -> Result<Vec<(String, String)>> {
    let package_name = string_field(package, "name")?;
    let existing = std::fs::read_to_string(repo_path.join(data_path(package_name))).ok();
    generated_files_from(
        repo_path,
        package,
        questions_path,
        file,
        existing.as_deref(),
    )
}

/// [`generated_files`] with an explicit baseline for the kept versions:
/// `ont release` passes the data file as the previous release shipped it (or
/// `None` when it shipped none), so an entry new since that release is stamped
/// with the new version even if a pre-release copy carried another.
pub fn generated_files_from(
    repo_path: &Path,
    package: &Value,
    questions_path: &str,
    file: &QuestionsFile,
    baseline: Option<&str>,
) -> Result<Vec<(String, String)>> {
    let package_name = string_field(package, "name")?;
    let version = string_field(package, "version")?;
    let records = stored_records(repo_path, package_name, file)?;
    let data = data_path(package_name);
    let existing = baseline;
    Ok(vec![
        (SCHEMA_PATH.to_string(), SCHEMA_TEXT.to_string()),
        (
            data.clone(),
            render_data(package_name, version, questions_path, &records, existing),
        ),
    ])
}

/// Writes the generated files, returning their paths.
pub fn write_generated(
    repo_path: &Path,
    package: &Value,
    questions_path: &str,
    file: &QuestionsFile,
) -> Result<Vec<PathBuf>> {
    let files = generated_files(repo_path, package, questions_path, file)?;
    write_files(repo_path, files)
}

/// Writes the generated files with versions kept from `baseline` (see
/// [`generated_files_from`]), returning their paths.
pub fn write_generated_from(
    repo_path: &Path,
    package: &Value,
    questions_path: &str,
    file: &QuestionsFile,
    baseline: Option<&str>,
) -> Result<Vec<PathBuf>> {
    let files = generated_files_from(repo_path, package, questions_path, file, baseline)?;
    write_files(repo_path, files)
}

fn write_files(repo_path: &Path, files: Vec<(String, String)>) -> Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    for (relative, contents) in files {
        let path = repo_path.join(&relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| Error::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        std::fs::write(&path, contents).map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        written.push(path);
    }
    Ok(written)
}

/// Checks that the package registers the generated files (schema entry, data
/// entry, load order) and that both files are what would be generated now.
pub fn validate_generated(
    repo_path: &Path,
    package: &Value,
    questions_path: &str,
    file: &QuestionsFile,
) -> Result<()> {
    let package_name = string_field(package, "name")?;
    let data = data_path(package_name);
    let apply_units_prefix = format!("{APPLY_UNITS_ROOT}/{}/", data.trim_end_matches(".tql"));
    let lists_data = |paths: &[&str]| {
        paths
            .iter()
            .any(|path| *path == data || path.starts_with(&apply_units_prefix))
    };

    let has_schema = package
        .get("schemas")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|schema| {
            schema.get("name").and_then(Value::as_str) == Some(SCHEMA_NAME)
                && schema.get("file").and_then(Value::as_str) == Some(SCHEMA_PATH)
        });
    if !has_schema {
        return Err(invalid(format!(
            "package.json 'schemas' must list {{\"name\": \"{SCHEMA_NAME}\", \"file\": \"{SCHEMA_PATH}\"}} \
             so the competency questions install"
        )));
    }
    if !lists_data(&string_array(package.get("data"))) {
        return Err(invalid(format!(
            "package.json 'data' must list {data} so the competency questions install"
        )));
    }
    if let Some(assembly) = package.get("assembly").filter(|value| value.is_object()) {
        let load_order = string_array(assembly.get("loadOrder"));
        if !load_order.contains(&SCHEMA_PATH) {
            return Err(invalid(format!(
                "assembly.loadOrder must list {SCHEMA_PATH}"
            )));
        }
        if !lists_data(&load_order) {
            return Err(invalid(format!("assembly.loadOrder must list {data}")));
        }
    }

    for (relative, expected) in generated_files(repo_path, package, questions_path, file)? {
        match std::fs::read_to_string(repo_path.join(&relative)) {
            Ok(current) if current == expected => {}
            Ok(_) => {
                return Err(invalid(format!(
                    "{relative} differs from what {questions_path} generates; {GENERATE_HINT}"
                )))
            }
            Err(_) => return Err(invalid(format!("{relative} not found; {GENERATE_HINT}"))),
        }
    }
    Ok(())
}

fn string_field<'a>(package: &'a Value, field: &str) -> Result<&'a str> {
    package
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| invalid(format!("package.json '{field}' must be a non-empty string")))
}

fn string_array(value: Option<&Value>) -> Vec<&str> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::competency_questions::Question;

    fn question(number: u64, title: Option<&str>, query: &str) -> Question {
        Question {
            id: format!("q{number:02}"),
            number,
            group: "Things".to_string(),
            text: format!("Question {number}?"),
            query: query.to_string(),
            title: title.map(str::to_string),
            explanation: Some(format!("Reads \"things\" for {number}.")),
            expected_empty: None,
        }
    }

    fn fixture() -> (tempfile::TempDir, QuestionsFile) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("queries")).unwrap();
        std::fs::write(
            dir.path().join("queries/a.tql"),
            "# 3. A question\n# second comment\n\nmatch $x isa thing;\n  select $x;\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("queries/b.tql"), "match $y isa thing;\n").unwrap();
        let file = QuestionsFile {
            questions: vec![
                question(3, Some("Which things exist?"), "queries/a.tql"),
                question(12, Some("Which things exist"), "queries/b.tql"),
            ],
        };
        (dir, file)
    }

    fn package(version: &str) -> Value {
        json!({
            "name": "custom",
            "version": version,
            "schemas": [{"name": "stored-queries", "file": "schema/stored-queries.tql"}],
            "data": ["data/custom-questions.tql"],
            "assembly": {"loadOrder": ["schema/stored-queries.tql", "data/custom-questions.tql"]}
        })
    }

    #[test]
    fn slug_matches_the_app() {
        assert_eq!(
            slug("Who's on first, and why?", "q-1"),
            "who-s-on-first-and-why"
        );
        assert_eq!(slug("!!!", "q-7"), "q-7");
        let long = "word ".repeat(30);
        let result = slug(&long, "q-1");
        assert!(result.len() <= 60 && result.ends_with("word"));
    }

    #[test]
    fn literal_escapes_like_the_app() {
        assert_eq!(literal("a\"b\\c\nd\te"), "\"a\\\"b\\\\c\\nd\\te\"");
    }

    #[test]
    fn records_use_the_app_keys_and_strip_leading_comments() {
        let (dir, file) = fixture();
        let records = stored_records(dir.path(), "custom", &file).unwrap();
        assert_eq!(records[0].question_id, "custom/q-03");
        assert_eq!(records[0].query_name, "custom/which-things-exist");
        assert_eq!(records[0].query_text, "match $x isa thing;\n  select $x;");
        assert_eq!(records[1].question_id, "custom/q-12");
        assert_eq!(records[1].query_name, "custom/which-things-exist-2");
    }

    #[test]
    fn renders_the_app_shape() {
        let (dir, file) = fixture();
        let records = stored_records(dir.path(), "custom", &file).unwrap();
        let text = render_data(
            "custom",
            "1.0.0",
            "competency/questions.json",
            &records[..1],
            None,
        );
        assert_eq!(
            text,
            "# Generated from competency/questions.json by `ont check-questions --write-generated`. Do not edit by hand.\n\
             # One StoredQuery and one StoredQuestion per competency question, joined by storedAnswer.\n\
             \n\
             put $query1 isa StoredQuery, has storedQueryName \"custom/which-things-exist\";\n\
             update $query1 has storedQueryTitle \"Which things exist?\";\n\
             \x20 $query1 has storedQueryText \"match $x isa thing;\\n  select $x;\";\n\
             \x20 $query1 has storedPackage \"custom\";\n\
             \x20 $query1 has storedVersion \"1.0.0\";\n\
             \x20 $query1 has storedQueryExplanation \"Reads \\\"things\\\" for 3.\";\n\
             put $question1 isa StoredQuestion, has storedQuestionId \"custom/q-03\";\n\
             update $question1 has storedQuestionText \"Question 3?\";\n\
             \x20 $question1 has storedPackage \"custom\";\n\
             \x20 $question1 has storedVersion \"1.0.0\";\n\
             \x20 $question1 has storedQuestionOrder 3;\n\
             put storedAnswer (question: $question1, query: $query1);\n"
        );
    }

    #[test]
    fn keeps_versions_of_unchanged_entries_and_stamps_changed_ones() {
        let (dir, mut file) = fixture();
        // Distinct titles, so reordering does not move the de-duplicating suffix.
        file.questions[1].title = Some("Other things".to_string());
        let records = stored_records(dir.path(), "custom", &file).unwrap();
        let first = render_data("custom", "1.0.0", "q.json", &records, None);

        // Nothing changed: a later version keeps every entry at 1.0.0.
        assert_eq!(
            render_data("custom", "1.0.1", "q.json", &records, Some(&first)),
            first
        );

        // Question 12's text changes and the order of questions flips.
        file.questions[1].text = "Changed?".to_string();
        file.questions.reverse();
        let records = stored_records(dir.path(), "custom", &file).unwrap();
        let second = render_data("custom", "1.0.2", "q.json", &records, Some(&first));
        let versions: Vec<(String, String)> = second
            .split("\n\n")
            .filter_map(|paragraph| {
                let id = question_id_regex().captures(paragraph)?[1].to_string();
                let version = version_regex().captures(paragraph)?[1].to_string();
                Some((id, version))
            })
            .collect();
        assert_eq!(
            versions,
            vec![
                ("custom/q-12".to_string(), "1.0.2".to_string()),
                ("custom/q-03".to_string(), "1.0.0".to_string()),
            ]
        );
    }

    #[test]
    fn validates_registration_and_freshness() {
        let (dir, file) = fixture();
        let check = |package: &Value| match validate_generated(dir.path(), package, "q.json", &file)
        {
            Err(Error::Version(message)) => message,
            other => panic!("expected an error, got {other:?}"),
        };
        let mut missing_schema = package("1.0.0");
        missing_schema["schemas"] = json!([]);
        assert_eq!(
            check(&missing_schema),
            "package.json 'schemas' must list {\"name\": \"stored-queries\", \"file\": \
             \"schema/stored-queries.tql\"} so the competency questions install"
        );
        let mut missing_data = package("1.0.0");
        missing_data["data"] = json!([]);
        assert_eq!(
            check(&missing_data),
            "package.json 'data' must list data/custom-questions.tql so the competency questions install"
        );
        let mut missing_order = package("1.0.0");
        missing_order["assembly"]["loadOrder"] = json!(["schema/stored-queries.tql"]);
        assert_eq!(
            check(&missing_order),
            "assembly.loadOrder must list data/custom-questions.tql"
        );
        assert_eq!(
            check(&package("1.0.0")),
            "schema/stored-queries.tql not found; run `ont check-questions --repo <repo> --write-generated`"
        );

        write_generated(dir.path(), &package("1.0.0"), "q.json", &file).unwrap();
        validate_generated(dir.path(), &package("1.0.0"), "q.json", &file).unwrap();
        // A later version with nothing changed still validates.
        validate_generated(dir.path(), &package("1.0.1"), "q.json", &file).unwrap();

        std::fs::write(dir.path().join("queries/b.tql"), "match $z isa thing;\n").unwrap();
        assert_eq!(
            check(&package("1.0.0")),
            "data/custom-questions.tql differs from what q.json generates; run `ont \
             check-questions --repo <repo> --write-generated`"
        );
    }

    #[test]
    fn a_release_stamps_entries_new_since_the_previous_release() {
        let (dir, file) = fixture();
        // A pre-release copy generated while the package said 1.0.0 …
        write_generated(dir.path(), &package("1.0.0"), "q.json", &file).unwrap();
        // … released as 1.1.0 when the previous release shipped no questions.
        let files =
            generated_files_from(dir.path(), &package("1.1.0"), "q.json", &file, None).unwrap();
        assert!(files[1].1.contains("storedVersion \"1.1.0\"") && !files[1].1.contains("1.0.0"));
        // Against a previous release that shipped the same entries, they keep 1.0.0.
        let shipped =
            std::fs::read_to_string(dir.path().join("data/custom-questions.tql")).unwrap();
        let files = generated_files_from(
            dir.path(),
            &package("1.1.0"),
            "q.json",
            &file,
            Some(&shipped),
        )
        .unwrap();
        assert!(!files[1].1.contains("1.1.0"));
    }

    #[test]
    fn each_question_is_one_executable_block() {
        let (dir, file) = fixture();
        let records = stored_records(dir.path(), "custom", &file).unwrap();
        let text = render_data("custom", "1.0.0", "q.json", &records, None);
        let blocks = crate::executable_package::split_executable_blocks(&text);
        assert_eq!(blocks.len(), records.len());
        assert!(blocks[1].starts_with("put $query2 ") && blocks[1].ends_with("query: $query2);"));
    }

    #[test]
    fn accepts_split_apply_units_for_the_data_file() {
        let (dir, file) = fixture();
        let mut package = package("1.0.0");
        package["data"] = json!(["generated/apply-units/data/custom-questions/0001.tql"]);
        package["assembly"]["loadOrder"] = json!([
            "schema/stored-queries.tql",
            "generated/apply-units/data/custom-questions/0001.tql"
        ]);
        write_generated(dir.path(), &package, "q.json", &file).unwrap();
        validate_generated(dir.path(), &package, "q.json", &file).unwrap();
    }
}
