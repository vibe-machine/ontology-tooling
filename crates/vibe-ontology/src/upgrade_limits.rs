//! Limits an upgrade (`migrations/`) file must meet so TypeDB and the app can
//! run it.
//!
//! The app (OneWorkspace Lingo, `OntologyViewModel+TypeDB.swift`
//! `applyMigrationDiff`) sends a `migrations/v<from>-to-v<to>.tql` file to
//! TypeDB as **one** write query. TypeDB rejects a query pipeline with more
//! than [`MAX_PIPELINE_STAGES`] stages (`QEX18`), so a file over that limit
//! can never run. An upgrade split into ordered units
//! (`migrations/v<from>-to-v<to>/NNNN.tql`) is held to the same limits as a
//! package's data write units (block count and size, see
//! [`crate::executable_package`]) plus the stage limit.

use std::fs;
use std::path::Path;

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

/// Counts the stages TypeDB sees when `text` is sent as one query: every
/// stage keyword (`match`, `insert`, `put`, `delete`, `update`, `fetch`,
/// `reduce`, `sort`, `offset`, `limit`, `select`, `require`, `distinct`) that
/// begins a clause at the top level. Comments, string literals and anything
/// inside brackets are skipped.
pub fn count_pipeline_stages(text: &str) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0;
    let mut depth: usize = 0;
    let mut clause_start = true;
    let mut stages = 0;

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
                    stages += 1;
                }
                clause_start = false;
                continue;
            }
            _ => clause_start = false,
        }
        index += 1;
    }
    stages
}

/// Checks a single-file upgrade (`migrations/v<from>-to-v<to>.tql`), which the
/// app runs as one query.
pub fn check_upgrade_query(relative_path: &str, text: &str) -> Result<()> {
    let stages = count_pipeline_stages(text);
    if stages == 0 {
        return Err(invalid(format!(
            "upgrade file '{relative_path}' contains no query stages"
        )));
    }
    if stages > MAX_PIPELINE_STAGES {
        return Err(invalid(format!(
            "upgrade file '{relative_path}' is one query of {stages} stages; TypeDB runs at most \
             {MAX_PIPELINE_STAGES} stages in one query (QEX18) and the app sends the whole file as \
             one query. Make it smaller, or move it into ordered units \
             migrations/<same name>/NNNN.tql, as `ont release` does for a generated upgrade."
        )));
    }
    Ok(())
}

/// Checks one unit of a split upgrade (`migrations/v<from>-to-v<to>/NNNN.tql`):
/// the data write-unit limits plus the stage limit.
pub fn check_upgrade_unit(relative_path: &str, text: &str) -> Result<()> {
    assert_safe_write_unit(relative_path, text)?;
    let stages = count_pipeline_stages(text);
    if stages > MAX_PIPELINE_STAGES {
        return Err(invalid(format!(
            "upgrade unit '{relative_path}' is one query of {stages} stages; TypeDB runs at most \
             {MAX_PIPELINE_STAGES} stages in one query (QEX18)"
        )));
    }
    Ok(())
}

/// Parses `v<from>-to-v<to>` (the app's upgrade-file name, without `.tql`).
fn is_upgrade_name(stem: &str) -> bool {
    fn is_version(text: &str) -> bool {
        let parts: Vec<&str> = text.split('.').collect();
        parts.len() == 3
            && parts
                .iter()
                .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
    }
    stem.strip_prefix('v')
        .and_then(|rest| rest.split_once("-to-v"))
        .is_some_and(|(from, to)| is_version(from) && is_version(to))
}

/// Validates every upgrade in `<repo>/migrations`: each
/// `v<from>-to-v<to>.tql` file must be runnable as one query, and each
/// `v<from>-to-v<to>/NNNN.tql` unit must meet the write-unit limits. Other
/// files (preflight/verify assertions, hand-written plan units) are left to
/// the migration contract.
pub fn validate_upgrade_files(repo_path: &Path) -> Result<()> {
    let migrations = repo_path.join("migrations");
    let Ok(entries) = fs::read_dir(&migrations) else {
        return Ok(());
    };
    let mut entries: Vec<_> = entries.filter_map(std::result::Result::ok).collect();
    entries.sort_by_key(fs::DirEntry::file_name);

    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if path.is_file() {
            if let Some(stem) = name.strip_suffix(".tql") {
                if is_upgrade_name(stem) {
                    let text = read(&path)?;
                    check_upgrade_query(&format!("migrations/{name}"), &text)?;
                }
            }
        } else if path.is_dir() && is_upgrade_name(&name) {
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
                check_upgrade_unit(&format!("migrations/{name}/{unit_name}"), &text)?;
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
    fn upgrade_query_over_the_stage_limit_is_rejected_with_an_exact_message() {
        let text = "put $x isa T;\n".repeat(MAX_PIPELINE_STAGES + 1);
        let error = check_upgrade_query("migrations/v1.0.0-to-v1.0.1.tql", &text)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(
                "upgrade file 'migrations/v1.0.0-to-v1.0.1.tql' is one query of 1001 stages; \
                 TypeDB runs at most 1000 stages in one query (QEX18)"
            ),
            "{error}"
        );
        let fits = "put $x isa T;\n".repeat(MAX_PIPELINE_STAGES);
        check_upgrade_query("migrations/v1.0.0-to-v1.0.1.tql", &fits).unwrap();
    }

    #[test]
    fn upgrade_unit_is_held_to_write_unit_limits() {
        let text: String = (0..51)
            .map(|index| format!("put $x{index} isa T, has name \"{index}\";\n\n"))
            .collect();
        let error = check_upgrade_unit("migrations/v1.0.0-to-v1.0.1/0001.tql", &text)
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
    fn validates_upgrade_files_and_unit_directories_only() {
        let directory = tempfile::tempdir().unwrap();
        let migrations = directory.path().join("migrations");
        fs::create_dir_all(migrations.join("verify")).unwrap();
        fs::create_dir_all(migrations.join("v1.0.1-to-v1.0.2")).unwrap();
        fs::write(migrations.join("v1.0.0-to-v1.0.1.tql"), "put $x isa T;\n").unwrap();
        fs::write(
            migrations.join("v1.0.1-to-v1.0.2/0001.tql"),
            "put $x isa T;\n",
        )
        .unwrap();
        // Assertions and other files are not upgrade files.
        fs::write(
            migrations.join("verify/assert-v1.0.1-module-version.tql"),
            "match $x isa T;\n".repeat(1200),
        )
        .unwrap();
        validate_upgrade_files(directory.path()).unwrap();

        fs::write(
            migrations.join("v1.0.0-to-v1.0.1.tql"),
            "put $x isa T;\n".repeat(1203),
        )
        .unwrap();
        let error = validate_upgrade_files(directory.path())
            .unwrap_err()
            .to_string();
        assert!(error.contains("is one query of 1203 stages"), "{error}");
    }

    #[test]
    fn recognises_only_the_apps_upgrade_names() {
        assert!(is_upgrade_name("v0.1.9-to-v1.0.0"));
        assert!(!is_upgrade_name("v0.1.4-to-v0.1.7-schema"));
        assert!(!is_upgrade_name("assert-v1.0.0-module-version"));
    }
}
