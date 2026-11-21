//! Sample data: the records a package's competency questions are answered
//! from, in the form the app loads them.
//!
//! The app (OneWorkspace, Lingo) builds a database `sample-<package>` from the
//! package, its dependencies and the files `package.json` lists under
//! `sampleData`, and offers a Run button on each question only when that list
//! is non-empty (`PackageSampleDatabase.swift`, `QuestionRunModel.swift`). It
//! applies each listed file as **one** write query in its own transaction, in
//! listed order, and refuses a list with a duplicate, a missing file, or a
//! file that is also installable (`schemas`, `data`, `assembly.loadOrder`)
//! (`PackageValidator.swift`, `validateSampleData`). Sample data is never
//! installed with the package.
//!
//! The layout follows the packages the app's Lab writes (beads):
//! `sandbox/sample-data/NNNN.tql`, one query per file. A package may instead
//! keep one source, `sandbox/sample-data.tql`, with its queries separated by
//! lines reading `end;` (as in the TypeDB console); `ont prepare-package` and
//! `ont release` then write the numbered files from it and list them, so
//! nobody splits them by hand, and validation refuses units that differ from
//! what the source generates.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::Value;

use crate::competency_questions::CompetencyDeclaration;
use crate::error::{Error, Result};
use crate::executable_package::assert_safe_write_unit;
use crate::upgrade_limits::{pipeline_stages, MAX_PIPELINE_STAGES};

/// The `package.json` field the app reads.
pub const FIELD: &str = "sampleData";
/// Where the numbered sample files live.
pub const UNITS_DIR: &str = "sandbox/sample-data";
/// The optional single source the numbered files are generated from.
pub const SOURCE_PATH: &str = "sandbox/sample-data.tql";

const GENERATED_HEADER: &str = "# Generated from sandbox/sample-data.tql by `ont prepare-package`; edit the source, not this file.";

fn invalid<S: Into<String>>(message: S) -> Error {
    Error::Version(message.into())
}

/// The listed sample files, in order (empty when none are listed).
pub fn sample_data_paths(package: &Value) -> Vec<String> {
    package
        .get(FIELD)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// The path of the `index`-th (0-based) numbered sample file.
pub fn unit_path(index: usize) -> String {
    format!("{UNITS_DIR}/{:04}.tql", index + 1)
}

/// Splits a sample source into its queries: lines reading `end;` separate
/// them. Comment lines before a query stay with it; a part holding only
/// comments and blank lines is dropped.
pub fn split_source(text: &str) -> Vec<String> {
    let mut queries = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut flush = |current: &mut Vec<&str>| {
        let part = current.join("\n").trim().to_string();
        let has_query = part
            .lines()
            .any(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'));
        if has_query {
            queries.push(part);
        }
        current.clear();
    };
    for line in text.lines() {
        if line.trim() == "end;" {
            flush(&mut current);
        } else {
            current.push(line);
        }
    }
    flush(&mut current);
    queries
}

/// The numbered files `sandbox/sample-data.tql` generates: `(path, text)`.
pub fn render_units(source: &str) -> Vec<(String, String)> {
    split_source(source)
        .into_iter()
        .enumerate()
        .map(|(index, query)| (unit_path(index), format!("{GENERATED_HEADER}\n{query}\n")))
        .collect()
}

/// When the package has `sandbox/sample-data.tql`, rewrites
/// `sandbox/sample-data/` from it (removing numbered files it no longer
/// generates) and sets `sampleData` to the generated list. Returns whether it
/// did. Called by `ont prepare-package` and `ont release`.
pub fn write_units_from_source(repo_path: &Path, package: &mut Value) -> Result<bool> {
    let source_path = repo_path.join(SOURCE_PATH);
    if !source_path.is_file() {
        return Ok(false);
    }
    let source = std::fs::read_to_string(&source_path).map_err(|source| Error::Io {
        path: source_path.clone(),
        source,
    })?;
    let units = render_units(&source);
    if units.is_empty() {
        return Err(invalid(format!("{SOURCE_PATH} holds no query")));
    }
    let directory = repo_path.join(UNITS_DIR);
    if directory.is_dir() {
        let entries = std::fs::read_dir(&directory).map_err(|source| Error::Io {
            path: directory.clone(),
            source,
        })?;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let numbered = name
                .strip_suffix(".tql")
                .is_some_and(|stem| !stem.is_empty() && stem.bytes().all(|b| b.is_ascii_digit()));
            if numbered {
                std::fs::remove_file(entry.path()).map_err(|source| Error::Io {
                    path: entry.path(),
                    source,
                })?;
            }
        }
    }
    std::fs::create_dir_all(&directory).map_err(|source| Error::Io {
        path: directory.clone(),
        source,
    })?;
    for (relative, text) in &units {
        let path = repo_path.join(relative);
        std::fs::write(&path, text).map_err(|source| Error::Io { path, source })?;
    }
    let listed = Value::Array(
        units
            .into_iter()
            .map(|(path, _)| Value::String(path))
            .collect(),
    );
    if let Some(object) = package.as_object_mut() {
        object.insert(FIELD.to_string(), listed);
    }
    Ok(true)
}

/// Checks one sample file the way the app will apply it: one write query,
/// within the data write-unit limits, at most one `match` (every later stage
/// runs once per row a `match` produced), and no more stages than TypeDB
/// allows in one query.
pub fn check_unit(relative_path: &str, text: &str) -> Result<()> {
    let fix = format!(
        "the app applies each sample file as one query in one transaction; split it into \
         several files (in {SOURCE_PATH}, separate queries with a line reading `end;`)"
    );
    if text.lines().any(|line| line.trim() == "end;") {
        return Err(invalid(format!(
            "sampleData file {relative_path} holds more than one query ('end;'); {fix}"
        )));
    }
    let stages = pipeline_stages(text);
    let first_word = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .and_then(|line| line.split_whitespace().next())
        .unwrap_or_default();
    if matches!(first_word, "define" | "undefine" | "redefine") {
        return Err(invalid(format!(
            "sampleData file {relative_path} is a schema query ('{first_word}'); sample data holds \
             only write queries, the schema belongs in the package"
        )));
    }
    let Some(first) = stages.first() else {
        return Err(invalid(format!(
            "sampleData file {relative_path} holds no query"
        )));
    };
    if !stages.iter().any(|stage| {
        matches!(
            stage.keyword.as_str(),
            "insert" | "put" | "update" | "delete"
        )
    }) {
        return Err(invalid(format!(
            "sampleData file {relative_path} writes nothing (it starts with '{}'); sample data \
             files are write queries",
            first.keyword
        )));
    }
    let matches = stages
        .iter()
        .filter(|stage| stage.keyword == "match")
        .count();
    if matches > 1 {
        return Err(invalid(format!(
            "sampleData file {relative_path} holds {matches} 'match' stages; every stage after a \
             'match' runs once per row it found, so rows multiply. Keep one 'match' per file; {fix}"
        )));
    }
    if stages.len() > MAX_PIPELINE_STAGES {
        return Err(invalid(format!(
            "sampleData file {relative_path} has {} stages; TypeDB allows {MAX_PIPELINE_STAGES} in \
             one query (QEX18); {fix}",
            stages.len()
        )));
    }
    assert_safe_write_unit(relative_path, text)
        .map_err(|error| invalid(format!("sampleData: {}; {fix}", plain(error))))
}

fn plain(error: Error) -> String {
    match error {
        Error::Version(message) => message,
        other => other.to_string(),
    }
}

/// The static sample-data check, part of `ont validate-package`:
///
/// - a package with competency questions (not waived) must list sample data,
///   because the app runs its questions only against it;
/// - `sampleData` must be an array of distinct, existing paths, none also
///   installable (`schemas`, `data`, `assembly.loadOrder`), as the app
///   requires;
/// - each file must pass [`check_unit`];
/// - when `sandbox/sample-data.tql` exists, the listed files must be exactly
///   what it generates.
pub fn validate_sample_data(
    repo_path: &Path,
    package: &Value,
    declaration: &CompetencyDeclaration,
) -> Result<()> {
    let listed = match package.get(FIELD) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(entries)) => {
            let mut paths = Vec::new();
            for (index, entry) in entries.iter().enumerate() {
                match entry.as_str().map(str::trim) {
                    Some(path) if !path.is_empty() => paths.push(path.to_string()),
                    _ => {
                        return Err(invalid(format!(
                            "package.json '{FIELD}' entry {} must be a non-empty path",
                            index + 1
                        )))
                    }
                }
            }
            paths
        }
        Some(_) => {
            return Err(invalid(format!(
                "package.json '{FIELD}' must be an array of paths"
            )))
        }
    };

    if listed.is_empty() {
        if let CompetencyDeclaration::Declared { .. } = declaration {
            return Err(invalid(format!(
                "package.json declares competency questions but no '{FIELD}': the app runs a \
                 package's questions only against the sample data it lists there (a database \
                 sample-<package> built from the package and these files), so without it no \
                 question can be run. Put the sample records in {UNITS_DIR}/NNNN.tql, one query \
                 per file (or in one {SOURCE_PATH}, queries separated by `end;` lines, and run \
                 `ont prepare-package --repo <repo>`), and list them in '{FIELD}'."
            )));
        }
        return Ok(());
    }

    let mut installable: BTreeSet<String> = package
        .get("schemas")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|schema| schema.get("file").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    for pointer in ["/data", "/assembly/loadOrder"] {
        installable.extend(
            package
                .pointer(pointer)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string),
        );
    }

    let mut seen = BTreeSet::new();
    for path in &listed {
        if !seen.insert(path.clone()) {
            return Err(invalid(format!(
                "package.json '{FIELD}' lists {path} twice"
            )));
        }
        if installable.contains(path) {
            return Err(invalid(format!(
                "package.json '{FIELD}' lists {path}, which is also in schemas, data or \
                 assembly.loadOrder; sample data is never installed with the package"
            )));
        }
        let full = repo_path.join(path.trim_start_matches('/'));
        if !full.is_file() {
            return Err(invalid(format!(
                "package.json '{FIELD}' lists {path}, which does not exist"
            )));
        }
        let text = std::fs::read_to_string(&full).map_err(|source| Error::Io {
            path: full.clone(),
            source,
        })?;
        check_unit(path, &text)?;
    }

    let source_path = repo_path.join(SOURCE_PATH);
    if source_path.is_file() {
        let source = std::fs::read_to_string(&source_path).map_err(|source| Error::Io {
            path: source_path.clone(),
            source,
        })?;
        let expected = render_units(&source);
        let regenerate = format!(
            "run `ont prepare-package --repo <repo>` to regenerate {UNITS_DIR}/ from {SOURCE_PATH}"
        );
        let expected_paths: Vec<&str> = expected.iter().map(|(path, _)| path.as_str()).collect();
        if expected_paths != listed.iter().map(String::as_str).collect::<Vec<_>>() {
            return Err(invalid(format!(
                "package.json '{FIELD}' does not list exactly the files {SOURCE_PATH} generates \
                 ({}); {regenerate}",
                expected_paths.join(", ")
            )));
        }
        for (path, text) in &expected {
            let current = std::fs::read_to_string(repo_path.join(path)).unwrap_or_default();
            if &current != text {
                return Err(invalid(format!(
                    "{path} differs from what {SOURCE_PATH} generates; {regenerate}"
                )));
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
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    fn declared() -> CompetencyDeclaration {
        CompetencyDeclaration::Declared {
            path: "competency/questions.json".to_string(),
            file: crate::competency_questions::QuestionsFile {
                questions: Vec::new(),
            },
        }
    }

    fn package(sample: Value) -> Value {
        json!({
            "name": "custom",
            "schemas": [{"name": "custom", "file": "schema/custom.tql"}],
            "data": ["data/seed.tql"],
            "sampleData": sample
        })
    }

    fn message(result: Result<()>) -> String {
        match result {
            Err(Error::Version(message)) => message,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn questions_need_sample_data_unless_waived() {
        let dir = tempfile::tempdir().unwrap();
        let mut package = package(json!([]));
        package.as_object_mut().unwrap().remove(FIELD);
        assert!(
            message(validate_sample_data(dir.path(), &package, &declared()))
                .starts_with("package.json declares competency questions but no 'sampleData'")
        );
        let waived = CompetencyDeclaration::Waived {
            reason: "none".to_string(),
        };
        validate_sample_data(dir.path(), &package, &waived).unwrap();
    }

    #[test]
    fn accepts_numbered_files_like_beads() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "sandbox/sample-data/0001.tql",
            "insert $x isa thing, has name \"a\";\n",
        );
        write(
            dir.path(),
            "sandbox/sample-data/0002.tql",
            "match $x isa thing, has name \"a\";\ninsert $y isa thing; rel (a: $x, b: $y);\n",
        );
        let package = package(json!([
            "sandbox/sample-data/0001.tql",
            "sandbox/sample-data/0002.tql"
        ]));
        validate_sample_data(dir.path(), &package, &declared()).unwrap();
    }

    #[test]
    fn refuses_what_the_app_refuses() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "data/seed.tql", "insert $x isa thing;\n");
        write(
            dir.path(),
            "sandbox/sample-data/0001.tql",
            "insert $x isa thing;\n",
        );
        assert_eq!(
            message(validate_sample_data(
                dir.path(),
                &package(json!([
                    "sandbox/sample-data/0001.tql",
                    "sandbox/sample-data/0001.tql"
                ])),
                &declared()
            )),
            "package.json 'sampleData' lists sandbox/sample-data/0001.tql twice"
        );
        assert_eq!(
            message(validate_sample_data(
                dir.path(),
                &package(json!(["data/seed.tql"])),
                &declared()
            )),
            "package.json 'sampleData' lists data/seed.tql, which is also in schemas, data or \
             assembly.loadOrder; sample data is never installed with the package"
        );
        assert_eq!(
            message(validate_sample_data(
                dir.path(),
                &package(json!(["sandbox/sample-data/0009.tql"])),
                &declared()
            )),
            "package.json 'sampleData' lists sandbox/sample-data/0009.tql, which does not exist"
        );
    }

    #[test]
    fn refuses_units_the_app_cannot_apply_as_one_query() {
        assert!(message(check_unit(
            "a.tql",
            "insert $x isa thing;\nend;\ninsert $y isa thing;\n"
        ))
        .contains("holds more than one query ('end;')"));
        assert!(message(check_unit("a.tql", "define\nentity thing;\n"))
            .contains("is a schema query ('define')"));
        assert!(message(check_unit("a.tql", "match $x isa thing;\n")).contains("writes nothing"));
        assert!(message(check_unit(
            "a.tql",
            "match $x isa thing;\ninsert $y isa thing;\nmatch $z isa thing;\ninsert $w isa thing;\n"
        ))
        .contains("holds 2 'match' stages"));
        let huge = format!(
            "insert $x isa thing, has name \"{}\";\n",
            "x".repeat(50_001)
        );
        assert!(message(check_unit("a.tql", &huge)).contains("max is 50000"));
    }

    #[test]
    fn source_generates_the_numbered_files_and_validation_holds_them_to_it() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            SOURCE_PATH,
            "# people\ninsert $x isa thing, has name \"a\";\nend;\n\n# links\nmatch $x isa thing, has name \"a\";\ninsert $y isa thing;\nend;\n# trailing note\n",
        );
        write(
            dir.path(),
            "sandbox/sample-data/0007.tql",
            "insert $old isa thing;\n",
        );
        let mut package = package(json!([]));
        assert!(write_units_from_source(dir.path(), &mut package).unwrap());
        assert_eq!(
            sample_data_paths(&package),
            vec![
                "sandbox/sample-data/0001.tql",
                "sandbox/sample-data/0002.tql"
            ]
        );
        assert!(!dir.path().join("sandbox/sample-data/0007.tql").exists());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("sandbox/sample-data/0002.tql")).unwrap(),
            format!("{GENERATED_HEADER}\n# links\nmatch $x isa thing, has name \"a\";\ninsert $y isa thing;\n")
        );
        validate_sample_data(dir.path(), &package, &declared()).unwrap();

        write(
            dir.path(),
            "sandbox/sample-data/0001.tql",
            "insert $x isa thing;\n",
        );
        assert!(
            message(validate_sample_data(dir.path(), &package, &declared())).starts_with(
                "sandbox/sample-data/0001.tql differs from what sandbox/sample-data.tql generates"
            )
        );
    }
}
