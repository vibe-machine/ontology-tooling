//! Migration-contract validation — the check that gates every ontology package's
//! `migration` block. Rust port of `src/lib/migration-contract.mjs`, so the CLI,
//! CI (one-2xg.18), and the app run the same validation (one-2xg.16 / one-2xg.46).
//!
//! Error messages are kept byte-for-byte with the JS so existing expectations
//! and tooling output are unchanged.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::version::{SemanticVersion, VersionRange};

fn invalid<S: Into<String>>(message: S) -> Error {
    Error::Version(message.into())
}

/// Validates the `migration` contract in `<repo_path>/package.json`. A package
/// with no `migration` block is valid (no-op). Returns the first violation as an
/// error, matching the JS `validateMigrationContract`.
pub fn validate_migration_contract(repo_path: &Path) -> Result<()> {
    let package_path = repo_path.join("package.json");
    let text = std::fs::read_to_string(&package_path)
        .map_err(|e| invalid(format!("failed to read {}: {e}", package_path.display())))?;
    let package: Value = serde_json::from_str(&text)
        .map_err(|e| invalid(format!("invalid json in {}: {e}", package_path.display())))?;

    let migration = match package.get("migration") {
        None | Some(Value::Null) => return Ok(()),
        Some(value) => value,
    };

    match migration.get("format") {
        Some(Value::Number(n)) if n.as_i64() == Some(1) => {}
        other => {
            return Err(invalid(format!(
                "migration.format must be 1, got {}",
                display_json(other)
            )));
        }
    }

    let plans = migration
        .get("plans")
        .and_then(Value::as_array)
        .filter(|plans| !plans.is_empty())
        .ok_or_else(|| invalid("migration.plans must contain at least one plan"))?;

    let plan_ids: Vec<String> = plans
        .iter()
        .map(|plan| {
            plan.get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        })
        .collect();
    assert_unique(&plan_ids, "migration plan ids")?;

    let declared_bootstrap_assets = package_asset_paths(&package);

    for plan in plans {
        validate_plan_shape(&package, plan)?;

        let plan_id = plan_id(plan);
        let mut saw_verify_phase = false;

        let phases = plan
            .get("phases")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for phase in &phases {
            let phase_id = phase.get("id").and_then(Value::as_str).unwrap_or("");
            if phase_id.trim().is_empty() {
                return Err(invalid(format!(
                    "migration plan '{plan_id}' contains a phase with an empty id"
                )));
            }

            let units = phase
                .get("units")
                .and_then(Value::as_array)
                .filter(|units| !units.is_empty())
                .ok_or_else(|| {
                    invalid(format!(
                        "migration plan '{plan_id}' phase '{phase_id}' must contain at least one unit"
                    ))
                })?;

            let mut phase_unit_paths = Vec::new();
            for unit in units {
                validate_unit_shape(plan, phase_id, unit)?;
                let kind = unit.get("kind").and_then(Value::as_str).unwrap_or("");
                let path = unit.get("path").and_then(Value::as_str).unwrap_or("");
                phase_unit_paths.push(format!("{kind}:{path}"));
                assert_path_exists(
                    repo_path,
                    path,
                    &format!("migration plan '{plan_id}' phase '{phase_id}'"),
                )?;
            }
            assert_unique(
                &phase_unit_paths,
                &format!("migration plan '{plan_id}' phase '{phase_id}' units"),
            )?;

            if phase_id == "verify"
                || units.iter().any(|unit| {
                    unit.get("kind")
                        .and_then(Value::as_str)
                        .is_some_and(|kind| kind.starts_with("assert-"))
                })
            {
                saw_verify_phase = true;
            }
        }

        if plan_mode(plan) == "replace" && !saw_verify_phase {
            return Err(invalid(format!(
                "migration plan '{plan_id}' uses replace mode and must include a verify/assert phase"
            )));
        }

        for asset in &declared_bootstrap_assets {
            if asset.starts_with("migrations/") {
                return Err(invalid(format!(
                    "bootstrap assets must not include migration-scoped files: {asset}"
                )));
            }
        }
    }

    Ok(())
}

fn plan_id(plan: &Value) -> String {
    plan.get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn plan_mode(plan: &Value) -> String {
    plan.get("mode")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn validate_plan_shape(package: &Value, plan: &Value) -> Result<()> {
    if !plan.is_object() {
        return Err(invalid("migration plan must be an object"));
    }

    let id = plan.get("id").and_then(Value::as_str).unwrap_or("");
    if id.trim().is_empty() {
        return Err(invalid("migration plan id must be a non-empty string"));
    }

    // `from` must be a valid semver range.
    let from = plan.get("from").and_then(Value::as_str).unwrap_or("");
    VersionRange::parse(from)?;

    let to = plan.get("to").and_then(Value::as_str).unwrap_or("");
    let target = SemanticVersion::parse(to)?;
    let package_version_str = package.get("version").and_then(Value::as_str).unwrap_or("");
    let package_version = SemanticVersion::parse(package_version_str)?;
    if target != package_version {
        return Err(invalid(format!(
            "migration plan '{id}' targets {to}, expected package version {package_version_str}"
        )));
    }

    let mode = plan.get("mode").and_then(Value::as_str).unwrap_or("");
    if mode != "replace" && mode != "compatible" {
        return Err(invalid(format!(
            "migration plan '{id}' has unsupported mode '{mode}'"
        )));
    }

    if mode == "replace" && !snapshot_required(plan) {
        return Err(invalid(format!(
            "migration plan '{id}' uses replace mode and must require a snapshot"
        )));
    }

    let phases = plan.get("phases").and_then(Value::as_array);
    match phases {
        Some(phases) if !phases.is_empty() => {
            let phase_ids: Vec<String> = phases
                .iter()
                .map(|phase| {
                    phase
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string()
                })
                .collect();
            assert_unique(&phase_ids, &format!("migration plan '{id}' phases"))?;
        }
        _ => {
            return Err(invalid(format!(
                "migration plan '{id}' must define at least one phase"
            )));
        }
    }

    Ok(())
}

fn snapshot_required(plan: &Value) -> bool {
    plan.get("snapshot")
        .and_then(|snapshot| snapshot.get("required"))
        .and_then(Value::as_bool)
        == Some(true)
}

fn validate_unit_shape(plan: &Value, phase_id: &str, unit: &Value) -> Result<()> {
    let id = plan_id(plan);
    if !unit.is_object() {
        return Err(invalid(format!(
            "migration plan '{id}' phase '{phase_id}' has a non-object unit"
        )));
    }

    let kind = unit.get("kind").and_then(Value::as_str).unwrap_or("");
    let valid = ["schema", "write", "assert-schema", "assert-data"];
    if !valid.contains(&kind) {
        return Err(invalid(format!(
            "migration plan '{id}' phase '{phase_id}' has unsupported unit kind '{kind}'"
        )));
    }

    let path = unit.get("path").and_then(Value::as_str).unwrap_or("");
    if path.trim().is_empty() {
        return Err(invalid(format!(
            "migration plan '{id}' phase '{phase_id}' has a unit with empty path"
        )));
    }

    let Some((from, to)) = versioned_migration_range(path) else {
        return Ok(());
    };

    let plan_to = SemanticVersion::parse(plan.get("to").and_then(Value::as_str).unwrap_or(""))?;
    if to != plan_to {
        return Err(invalid(format!(
            "migration plan '{id}' unit '{path}' must target version {}",
            plan.get("to").and_then(Value::as_str).unwrap_or("")
        )));
    }

    let plan_from = VersionRange::parse(plan.get("from").and_then(Value::as_str).unwrap_or(""))?;
    if !plan_from.satisfies(&from) {
        return Err(invalid(format!(
            "migration plan '{id}' unit '{path}' starts from {}.{}.{}, which is outside source range '{}'",
            from.major,
            from.minor,
            from.patch,
            plan.get("from").and_then(Value::as_str).unwrap_or("")
        )));
    }

    Ok(())
}

/// Parses a `<from>-to-<to>.tql` filename into its (from, to) versions, or
/// `None` if the filename isn't a versioned migration. Mirrors
/// `versionedMigrationRange`.
fn versioned_migration_range(relative_path: &str) -> Option<(SemanticVersion, SemanticVersion)> {
    let filename = relative_path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(relative_path);
    let stem = filename.strip_suffix(".tql")?;
    let (from_text, to_text) = stem.split_once("-to-")?;
    // Exactly two parts: reject filenames with more than one "-to-".
    if to_text.contains("-to-") {
        return None;
    }
    let from = SemanticVersion::parse(from_text.strip_prefix('v').unwrap_or(from_text)).ok()?;
    let to = SemanticVersion::parse(to_text.strip_prefix('v').unwrap_or(to_text)).ok()?;
    Some((from, to))
}

/// The union of a package's declared bootstrap asset paths (schemas, data,
/// manifests, provenance). Mirrors `packageAssetPaths`.
fn package_asset_paths(package: &Value) -> BTreeSet<String> {
    let mut set = BTreeSet::new();

    if let Some(schemas) = package.get("schemas").and_then(Value::as_array) {
        for entry in schemas {
            if let Some(file) = entry.get("file").and_then(Value::as_str) {
                set.insert(file.to_string());
            }
        }
    }
    for key in ["data", "manifests"] {
        if let Some(arr) = package.get(key).and_then(Value::as_array) {
            for entry in arr {
                if let Some(s) = entry.as_str() {
                    set.insert(s.to_string());
                }
            }
        }
    }

    match package.get("provenance") {
        Some(Value::Array(files)) => {
            for entry in files {
                if let Some(s) = entry.as_str() {
                    set.insert(s.to_string());
                }
            }
        }
        Some(Value::Object(_)) => {
            let provenance = &package["provenance"];
            if let Some(files) = provenance.get("files").and_then(Value::as_array) {
                for entry in files {
                    if let Some(s) = entry.as_str() {
                        set.insert(s.to_string());
                    }
                }
            } else if let Some(manifest) = provenance.get("manifest").and_then(Value::as_str) {
                set.insert(manifest.to_string());
            }
        }
        _ => {}
    }

    set
}

fn assert_path_exists(repo_path: &Path, relative_path: &str, context: &str) -> Result<()> {
    if repo_path.join(relative_path).exists() {
        Ok(())
    } else {
        Err(invalid(format!(
            "{context} references missing file: {relative_path}"
        )))
    }
}

fn assert_unique(values: &[String], context: &str) -> Result<()> {
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value.clone()) {
            return Err(invalid(format!(
                "{context} contains duplicate value: {value}"
            )));
        }
    }
    Ok(())
}

fn display_json(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "undefined".to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versioned_range_parses_from_to_filenames() {
        let (from, to) = versioned_migration_range("migrations/v2.0.4-to-v3.0.0.tql").unwrap();
        assert_eq!(from, SemanticVersion::new(2, 0, 4));
        assert_eq!(to, SemanticVersion::new(3, 0, 0));
        assert!(versioned_migration_range("migrations/structural-schema.tql").is_none());
        assert!(versioned_migration_range("notes.txt").is_none());
    }

    #[test]
    fn package_asset_paths_unions_all_sources() {
        let package: Value = serde_json::json!({
            "schemas": [{ "file": "schema/a.tql" }, { "file": "schema/b.tql" }],
            "data": ["data/seed.tql"],
            "provenance": { "manifest": "prov/manifest.json" }
        });
        let assets = package_asset_paths(&package);
        assert!(assets.contains("schema/a.tql"));
        assert!(assets.contains("data/seed.tql"));
        assert!(assets.contains("prov/manifest.json"));
    }

    fn write(dir: &std::path::Path, rel: &str, contents: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn no_migration_block_is_valid() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "package.json",
            r#"{ "name": "gist", "version": "3.0.0" }"#,
        );
        assert!(validate_migration_contract(dir.path()).is_ok());
    }

    #[test]
    fn valid_compatible_plan_passes() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "migrations/structural.tql",
            "define\nattribute a value string;",
        );
        write(
            dir.path(),
            "package.json",
            r#"{
              "name": "gist", "version": "3.0.0",
              "migration": { "format": 1, "plans": [{
                "id": "gist-structural", "from": "2.0.x", "to": "3.0.0", "mode": "compatible",
                "phases": [{ "id": "schema", "units": [{ "kind": "schema", "path": "migrations/structural.tql" }] }]
              }]}
            }"#,
        );
        validate_migration_contract(dir.path()).expect("valid plan should pass");
    }

    #[test]
    fn plan_target_must_match_package_version() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "migrations/structural.tql",
            "define\nattribute a value string;",
        );
        write(
            dir.path(),
            "package.json",
            r#"{
              "name": "gist", "version": "3.0.0",
              "migration": { "format": 1, "plans": [{
                "id": "p", "from": "2.0.x", "to": "2.9.9", "mode": "compatible",
                "phases": [{ "id": "schema", "units": [{ "kind": "schema", "path": "migrations/structural.tql" }] }]
              }]}
            }"#,
        );
        let err = validate_migration_contract(dir.path())
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("targets 2.9.9, expected package version 3.0.0"),
            "{err}"
        );
    }

    #[test]
    fn replace_mode_requires_snapshot_and_verify() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "migrations/structural.tql",
            "define\nattribute a value string;",
        );
        write(
            dir.path(),
            "package.json",
            r#"{
              "name": "gist", "version": "3.0.0",
              "migration": { "format": 1, "plans": [{
                "id": "p", "from": "2.0.x", "to": "3.0.0", "mode": "replace",
                "phases": [{ "id": "schema", "units": [{ "kind": "schema", "path": "migrations/structural.tql" }] }]
              }]}
            }"#,
        );
        let err = validate_migration_contract(dir.path())
            .unwrap_err()
            .to_string();
        assert!(err.contains("must require a snapshot"), "{err}");
    }

    #[test]
    fn missing_unit_file_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "package.json",
            r#"{
              "name": "gist", "version": "3.0.0",
              "migration": { "format": 1, "plans": [{
                "id": "p", "from": "2.0.x", "to": "3.0.0", "mode": "compatible",
                "phases": [{ "id": "schema", "units": [{ "kind": "schema", "path": "migrations/missing.tql" }] }]
              }]}
            }"#,
        );
        let err = validate_migration_contract(dir.path())
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("references missing file: migrations/missing.tql"),
            "{err}"
        );
    }

    #[test]
    fn versioned_unit_from_must_be_in_source_range() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "migrations/v1.0.0-to-v3.0.0.tql",
            "define\nattribute a value string;",
        );
        write(
            dir.path(),
            "package.json",
            r#"{
              "name": "gist", "version": "3.0.0",
              "migration": { "format": 1, "plans": [{
                "id": "p", "from": "2.0.x", "to": "3.0.0", "mode": "compatible",
                "phases": [{ "id": "schema", "units": [{ "kind": "schema", "path": "migrations/v1.0.0-to-v3.0.0.tql" }] }]
              }]}
            }"#,
        );
        let err = validate_migration_contract(dir.path())
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("starts from 1.0.0, which is outside source range '2.0.x'"),
            "{err}"
        );
    }
}
