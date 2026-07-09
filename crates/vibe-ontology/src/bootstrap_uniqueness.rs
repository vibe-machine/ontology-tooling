//! Bootstrap uniqueness validation for keyed TypeQL entities.
//!
//! Rust port of `src/lib/bootstrap-uniqueness.mjs`, so the CLI, CI
//! (one-2xg.18), and the app share one implementation
//! (one-2xg.16 / one-2xg.46). Match ordering, first-error behavior, and
//! validation messages intentionally match JavaScript.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;

use crate::error::{Error, Result};

fn invalid<S: Into<String>>(message: S) -> Error {
    Error::Version(message.into())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct KeyedEntity {
    entity_type: String,
    key_attrs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Violation {
    path: Option<String>,
    entity_type: String,
    key_attr: String,
    value: String,
}

fn entity_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?m)^entity\s+((?-u:\w)+)(?:\s+sub\s+(?-u:\w)+)?\s*,?\s*\n((?s:.*?));")
            .expect("entity regex is valid")
    })
}

fn key_attribute_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"owns\s+((?-u:\w)+)\s+@key").expect("key attribute regex is valid")
    })
}

fn extract_keyed_entities_from_schema(tql: &str) -> Vec<KeyedEntity> {
    entity_regex()
        .captures_iter(tql)
        .filter_map(|captures| {
            let key_attrs: Vec<String> = key_attribute_regex()
                .captures_iter(&captures[2])
                .map(|key_match| key_match[1].to_string())
                .collect();
            (!key_attrs.is_empty()).then(|| KeyedEntity {
                entity_type: captures[1].to_string(),
                key_attrs,
            })
        })
        .collect()
}

fn extract_insert_key_values(tql: &str, entity_type: &str, key_attr: &str) -> Vec<String> {
    let insert_pattern = Regex::new(&format!(
        r"insert\s+\$(?-u:\w)+\s+isa\s+{}\s*,((?s:.*?));",
        regex::escape(entity_type)
    ))
    .expect("escaped entity type produces a valid regex");
    let value_pattern = Regex::new(&format!(
        r#"has\s+{}\s+\"([^\"]*)\""#,
        regex::escape(key_attr)
    ))
    .expect("escaped key attribute produces a valid regex");

    insert_pattern
        .captures_iter(tql)
        .filter_map(|captures| {
            value_pattern
                .captures(&captures[1])
                .map(|value_match| value_match[1].to_string())
        })
        .collect()
}

fn find_insert_statements_for_keyed_entities(
    tql: &str,
    keyed_entities: &[KeyedEntity],
) -> Vec<Violation> {
    let mut violations = Vec::new();

    for keyed_entity in keyed_entities {
        for key_attr in &keyed_entity.key_attrs {
            for value in extract_insert_key_values(tql, &keyed_entity.entity_type, key_attr) {
                violations.push(Violation {
                    path: None,
                    entity_type: keyed_entity.entity_type.clone(),
                    key_attr: key_attr.clone(),
                    value,
                });
            }
        }
    }

    violations
}

fn format_violations(violations: &[Violation]) -> String {
    let rendered = violations
        .iter()
        .map(|violation| {
            format!(
                "- {}: uses insert for keyed entity {} via {}=\"{}\"; use put instead",
                violation.path.as_deref().unwrap_or("undefined"),
                violation.entity_type,
                violation.key_attr,
                violation.value
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("Bootstrap uniqueness validation failed:\n{rendered}")
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

fn string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect()
}

/// Rejects bootstrap assets that use `insert` for entities whose schema owns
/// a `@key`, matching `validateBootstrapUniqueness`.
pub fn validate_bootstrap_uniqueness(repo_path: &Path) -> Result<()> {
    let package = read_json(&repo_path.join("package.json"))?;
    let load_order = string_array(
        package
            .get("assembly")
            .and_then(|assembly| assembly.get("loadOrder")),
    );
    let schema_files: Vec<String> = package
        .get("schemas")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|schema| schema.get("file").and_then(Value::as_str))
        .map(ToOwned::to_owned)
        .collect();
    let mut keyed_entities: Vec<KeyedEntity> = Vec::new();

    for relative_path in &load_order {
        if !schema_files.contains(relative_path) {
            continue;
        }
        let tql = read_text(&joined_path(repo_path, relative_path))?;
        for keyed_entity in extract_keyed_entities_from_schema(&tql) {
            if let Some(existing) = keyed_entities
                .iter_mut()
                .find(|existing| existing.entity_type == keyed_entity.entity_type)
            {
                existing.key_attrs = keyed_entity.key_attrs;
            } else {
                keyed_entities.push(keyed_entity);
            }
        }
    }

    if keyed_entities.is_empty() {
        return Ok(());
    }

    let mut violations = Vec::new();
    for relative_path in &load_order {
        if schema_files.contains(relative_path) {
            continue;
        }
        let tql = read_text(&joined_path(repo_path, relative_path))?;
        for mut violation in find_insert_statements_for_keyed_entities(&tql, &keyed_entities) {
            violation.path = Some(relative_path.clone());
            violations.push(violation);
        }
    }

    if violations.is_empty() {
        Ok(())
    } else {
        Err(invalid(format_violations(&violations)))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;
    use tempfile::TempDir;

    use super::*;

    fn error_message(error: Error) -> String {
        match error {
            Error::Version(message) => message,
            other => panic!("expected version error, got {other}"),
        }
    }

    #[test]
    fn extract_keyed_entities_from_schema_returns_keyed_entity_attributes() {
        let schema = r#"define

attribute moduleVersionKey, value string;

entity OntologyModuleVersion,
  owns moduleVersionKey @key,
  owns moduleVersion;
"#;

        assert_eq!(
            extract_keyed_entities_from_schema(schema),
            vec![KeyedEntity {
                entity_type: "OntologyModuleVersion".to_string(),
                key_attrs: vec!["moduleVersionKey".to_string()],
            }]
        );
    }

    #[test]
    fn find_insert_statements_for_keyed_entities_reports_keyed_insert_violations() {
        let tql = r#"insert

$version isa OntologyModuleVersion,
  has moduleVersionKey "https://github.com/objectiveous/ontology-vibemachine@0.2.2",
  has moduleVersion "0.2.2";
"#;

        assert_eq!(
            find_insert_statements_for_keyed_entities(
                tql,
                &[KeyedEntity {
                    entity_type: "OntologyModuleVersion".to_string(),
                    key_attrs: vec!["moduleVersionKey".to_string()],
                }],
            ),
            vec![Violation {
                path: None,
                entity_type: "OntologyModuleVersion".to_string(),
                key_attr: "moduleVersionKey".to_string(),
                value: "https://github.com/objectiveous/ontology-vibemachine@0.2.2".to_string(),
            }]
        );
    }

    fn create_fixture_repo(broken_refresh: bool) -> TempDir {
        let directory = tempfile::tempdir().unwrap();
        let repo_path = directory.path();
        fs::create_dir_all(repo_path.join("schema")).unwrap();
        fs::create_dir_all(repo_path.join("data")).unwrap();
        fs::create_dir_all(repo_path.join("manifests")).unwrap();

        let package = json!({
            "name": "fixture-package",
            "schemas": [{
                "name": "package-provenance",
                "file": "schema/package-provenance.tql"
            }],
            "data": ["data/fixture-provenance.tql"],
            "manifests": ["manifests/fixture-package-v1.0.0.package-manifest.json"],
            "assembly": {
                "loadOrder": [
                    "schema/package-provenance.tql",
                    "data/fixture-provenance.tql",
                    "manifests/fixture-package-v1.0.0.package-manifest.json"
                ]
            }
        });
        fs::write(
            repo_path.join("package.json"),
            format!("{}\n", serde_json::to_string_pretty(&package).unwrap()),
        )
        .unwrap();
        fs::write(
            repo_path.join("schema/package-provenance.tql"),
            r#"define

attribute moduleRepoUrl, value string;
attribute moduleVersionKey, value string;

entity OntologyModule,
  owns moduleRepoUrl @key;

entity OntologyModuleVersion,
  owns moduleVersionKey @key,
  owns moduleVersion;
"#,
        )
        .unwrap();
        let command = if broken_refresh { "insert" } else { "put" };
        fs::write(
            repo_path.join("data/fixture-provenance.tql"),
            format!(
                r#"{command} $version isa OntologyModuleVersion,
  has moduleVersionKey "https://example.com/fixture-package@1.0.1",
  has moduleVersion "1.0.1";
"#
            ),
        )
        .unwrap();
        fs::write(
            repo_path.join("manifests/fixture-package-v1.0.0.package-manifest.json"),
            "{}\n",
        )
        .unwrap();

        directory
    }

    #[test]
    fn validate_bootstrap_uniqueness_accepts_put_for_keyed_entities() {
        let directory = create_fixture_repo(false);
        validate_bootstrap_uniqueness(directory.path()).unwrap();
    }

    #[test]
    fn release_output_that_inserts_keyed_entities_is_rejected() {
        let directory = create_fixture_repo(true);
        assert_eq!(
            error_message(validate_bootstrap_uniqueness(directory.path()).unwrap_err()),
            concat!(
                "Bootstrap uniqueness validation failed:\n",
                "- data/fixture-provenance.tql: uses insert for keyed entity ",
                "OntologyModuleVersion via moduleVersionKey=\"https://example.com/",
                "fixture-package@1.0.1\"; use put instead"
            )
        );
    }
}
