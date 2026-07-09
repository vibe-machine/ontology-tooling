//! Package-contract validation for self-describing ontology repositories.
//!
//! Rust port of `src/lib/package-validator.mjs`, so the CLI, CI
//! (one-2xg.18), and the app share one implementation
//! (one-2xg.16 / one-2xg.46). Validation order and error messages intentionally
//! match the JavaScript implementation.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::executable_package::{validate_executable_package, APPLY_UNITS_ROOT};
use crate::migration_contract::validate_migration_contract;
use crate::version::{SemanticVersion, VersionRange};

fn invalid<S: Into<String>>(message: S) -> Error {
    Error::Version(message.into())
}

/// Validates the package contract rooted at `repo_path`, returning the first
/// violation in the same order as the JavaScript `validatePackageContract`.
pub fn validate_package_contract(repo_path: &Path) -> Result<()> {
    let package_path = repo_path.join("package.json");
    let text = std::fs::read_to_string(&package_path).map_err(|error| {
        invalid(format!(
            "failed to read {}: {error}",
            package_path.display()
        ))
    })?;
    let package: Value = serde_json::from_str(&text).map_err(|error| {
        invalid(format!(
            "invalid json in {}: {error}",
            package_path.display()
        ))
    })?;

    validate_required_fields(&package)?;
    validate_release_scripts(&package)?;
    validate_schema_entries(repo_path, &package)?;
    validate_file_list(repo_path, package.get("data"), "data file")?;
    validate_file_list(repo_path, package.get("manifests"), "manifest file")?;

    match package.get("provenance") {
        Some(Value::Array(_)) => {
            validate_file_list(repo_path, package.get("provenance"), "provenance file")?;
        }
        Some(Value::Object(provenance)) => {
            if provenance.get("files").is_some_and(Value::is_array) {
                validate_file_list(repo_path, provenance.get("files"), "provenance file")?;
            } else if is_non_empty_string(provenance.get("manifest")) {
                let manifest = provenance
                    .get("manifest")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                assert_path_exists(repo_path, manifest, "provenance file")?;
            }
        }
        _ => {}
    }

    if is_non_empty_string(package.get("domains")) {
        assert_path_exists(
            repo_path,
            package.get("domains").and_then(Value::as_str).unwrap_or(""),
            "domains file",
        )?;
    }
    if is_non_empty_string(package.get("categorization")) {
        assert_path_exists(
            repo_path,
            package
                .get("categorization")
                .and_then(Value::as_str)
                .unwrap_or(""),
            "categorization file",
        )?;
    }

    validate_schema_docs_apply_units(&package)?;
    validate_assembly(repo_path, &package)?;
    validate_migration_contract(repo_path)?;
    validate_previous_release_coverage(repo_path, &package)?;
    validate_executable_package(repo_path, &package)?;

    Ok(())
}

fn is_non_empty_string(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
}

fn parse_semver(value: Option<&Value>) -> Result<SemanticVersion> {
    let Some(raw) = value.and_then(Value::as_str) else {
        return Err(invalid(format!(
            "invalid semver value: {}",
            display_js(value)
        )));
    };
    if raw.trim().is_empty() {
        return Err(invalid(format!("invalid semver value: {raw}")));
    }

    let trimmed = raw.trim();
    let version = trimmed.strip_prefix('v').unwrap_or(trimmed);
    SemanticVersion::parse(version).map_err(|_| invalid(format!("invalid semver value: {raw}")))
}

fn validate_required_fields(package: &Value) -> Result<()> {
    if !is_non_empty_string(package.get("name")) {
        return Err(invalid("package.json 'name' must be a non-empty string"));
    }
    if !is_non_empty_string(package.get("displayName")) {
        return Err(invalid(
            "package.json 'displayName' must be a non-empty string",
        ));
    }
    parse_semver(package.get("version"))?;
    if package
        .get("schemas")
        .and_then(Value::as_array)
        .is_none_or(Vec::is_empty)
    {
        return Err(invalid(
            "package.json 'schemas' must contain at least one schema entry",
        ));
    }
    Ok(())
}

fn required_release_scripts(package: &Value) -> Vec<&'static str> {
    let mut required = vec![
        "refresh:package-contract",
        "validate:bootstrap",
        "test:typedb-bootstrap",
    ];
    if package.get("migration").is_some_and(js_truthy) {
        required.push("test:typedb-migration");
    }
    required
}

fn validate_release_scripts(package: &Value) -> Result<()> {
    let scripts = package.get("scripts").filter(|value| !value.is_null());
    for script_name in required_release_scripts(package) {
        if !is_non_empty_string(scripts.and_then(|scripts| scripts.get(script_name))) {
            return Err(invalid(format!(
                "package.json scripts must define '{script_name}'"
            )));
        }
    }
    Ok(())
}

fn validate_schema_entries(repo_path: &Path, package: &Value) -> Result<()> {
    let schemas = package
        .get("schemas")
        .and_then(Value::as_array)
        .expect("required fields established a non-empty schemas array");
    for schema in schemas {
        if schema.is_null() || (!schema.is_object() && !schema.is_array()) {
            return Err(invalid("schema entries must be objects"));
        }
        if !is_non_empty_string(schema.get("name")) {
            return Err(invalid("schema entry name must be a non-empty string"));
        }
        if !is_non_empty_string(schema.get("file")) {
            return Err(invalid(format!(
                "schema '{}' file must be a non-empty string",
                schema.get("name").and_then(Value::as_str).unwrap_or("")
            )));
        }
        assert_path_exists(
            repo_path,
            schema.get("file").and_then(Value::as_str).unwrap_or(""),
            "schema file",
        )?;
    }
    Ok(())
}

fn validate_file_list(repo_path: &Path, paths: Option<&Value>, context: &str) -> Result<()> {
    let Some(paths) = paths.filter(|value| !value.is_null()) else {
        return Ok(());
    };
    let Some(paths) = paths.as_array() else {
        return Err(invalid(format!("{context} is not iterable")));
    };
    for relative_path in paths {
        if !is_non_empty_string(Some(relative_path)) {
            return Err(invalid(format!("{context} contains an empty path")));
        }
        assert_path_exists(repo_path, relative_path.as_str().unwrap_or(""), context)?;
    }
    Ok(())
}

fn package_asset_paths(package: &Value) -> Vec<Value> {
    let mut paths = Vec::new();
    if let Some(schemas) = package.get("schemas").and_then(Value::as_array) {
        paths.extend(
            schemas
                .iter()
                .map(|schema| schema.get("file").cloned().unwrap_or(Value::Null)),
        );
    }
    for key in ["data", "manifests"] {
        if let Some(values) = package.get(key).and_then(Value::as_array) {
            paths.extend(values.iter().cloned());
        }
    }
    match package.get("provenance") {
        Some(Value::Array(values)) => paths.extend(values.iter().cloned()),
        Some(Value::Object(provenance)) => {
            if let Some(values) = provenance.get("files").and_then(Value::as_array) {
                paths.extend(values.iter().cloned());
            } else if let Some(manifest) = provenance.get("manifest").and_then(Value::as_str) {
                paths.push(Value::String(manifest.to_string()));
            }
        }
        _ => {}
    }
    if let Some(values) = package
        .get("assembly")
        .and_then(|assembly| assembly.get("generatedArtifacts"))
        .and_then(Value::as_array)
    {
        paths.extend(values.iter().cloned());
    }
    paths
}

fn validate_assembly(repo_path: &Path, package: &Value) -> Result<()> {
    let Some(assembly) = package.get("assembly").filter(|value| js_truthy(value)) else {
        return Ok(());
    };
    let load_order = assembly
        .get("loadOrder")
        .and_then(Value::as_array)
        .filter(|paths| !paths.is_empty())
        .ok_or_else(|| invalid("assembly.loadOrder must contain at least one asset"))?;
    assert_unique_values(load_order, "assembly.loadOrder")?;

    let declared_assets = package_asset_paths(package);
    for relative_path in load_order {
        if !declared_assets.contains(relative_path) {
            return Err(invalid(format!(
                "assembly.loadOrder references undeclared asset: {}",
                display_js(Some(relative_path))
            )));
        }
        assert_path_exists(
            repo_path,
            relative_path.as_str().unwrap_or(""),
            "assembly asset",
        )?;
    }
    Ok(())
}

fn validate_previous_release_coverage(repo_path: &Path, package: &Value) -> Result<()> {
    let Some(plans) = package
        .get("migration")
        .and_then(|migration| migration.get("plans"))
        .and_then(Value::as_array)
        .filter(|plans| !plans.is_empty())
    else {
        return Ok(());
    };

    let current = parse_semver(package.get("version"))?;
    let Some(previous) = previous_release_version(repo_path, current)? else {
        return Ok(());
    };
    let mut covered = false;
    for plan in plans {
        let range = VersionRange::parse(plan.get("from").and_then(Value::as_str).unwrap_or(""))?;
        if range.satisfies(&previous) {
            covered = true;
            break;
        }
    }
    if !covered {
        return Err(invalid(format!(
            "migration plans do not cover previous release {previous} -> {}",
            display_js(package.get("version"))
        )));
    }
    Ok(())
}

fn previous_release_version(
    repo_path: &Path,
    current: SemanticVersion,
) -> Result<Option<SemanticVersion>> {
    let output = Command::new("git")
        .args(["tag", "--list", "v*.*.*"])
        .current_dir(repo_path)
        .output()
        .map_err(|error| invalid(error.to_string()))?;
    if !output.status.success() {
        return Err(invalid(String::from_utf8_lossy(&output.stderr).trim()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut versions = Vec::new();
    for tag in stdout.lines().map(str::trim).filter(|tag| !tag.is_empty()) {
        let tag_value = Value::String(tag.to_string());
        versions.push(parse_semver(Some(&tag_value))?);
    }
    versions.sort_unstable();
    Ok(versions.into_iter().rfind(|version| *version < current))
}

fn schema_docs_stem(relative_path: &str) -> Option<&str> {
    relative_path
        .strip_prefix("data/")?
        .strip_suffix("-schema-docs.tql")
        .filter(|stem| !stem.is_empty())
}

fn schema_docs_apply_unit_stem(relative_path: &str) -> Option<&str> {
    let remainder = relative_path.strip_prefix(&format!("{APPLY_UNITS_ROOT}/data/"))?;
    let (directory, filename) = remainder.rsplit_once('/')?;
    let number = filename.strip_suffix(".tql")?;
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    directory
        .strip_suffix("-schema-docs")
        .filter(|stem| !stem.is_empty())
}

fn validate_schema_docs_apply_units(package: &Value) -> Result<()> {
    let data_paths = string_array(package.get("data"));
    let load_order_paths = string_array(
        package
            .get("assembly")
            .and_then(|assembly| assembly.get("loadOrder")),
    );
    let executable_paths: Vec<&str> = data_paths.into_iter().chain(load_order_paths).collect();
    let apply_unit_stems: BTreeSet<&str> = executable_paths
        .iter()
        .filter_map(|path| schema_docs_apply_unit_stem(path))
        .collect();
    if apply_unit_stems.is_empty() {
        return Ok(());
    }
    for relative_path in executable_paths {
        if schema_docs_stem(relative_path).is_some_and(|stem| apply_unit_stems.contains(stem)) {
            return Err(invalid(format!(
                "schema docs '{relative_path}' must not be listed as executable data when split apply units exist"
            )));
        }
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

fn assert_path_exists(repo_path: &Path, relative_path: &str, context: &str) -> Result<()> {
    if joined_path(repo_path, relative_path).exists() {
        Ok(())
    } else {
        Err(invalid(format!("{context} not found: {relative_path}")))
    }
}

fn joined_path(repo_path: &Path, relative_path: &str) -> PathBuf {
    repo_path.join(relative_path.trim_start_matches('/'))
}

fn assert_unique_values(values: &[Value], context: &str) -> Result<()> {
    for (index, value) in values.iter().enumerate() {
        if values[..index].contains(value) {
            return Err(invalid(format!(
                "{context} contains duplicate value: {}",
                display_js(Some(value))
            )));
        }
    }
    Ok(())
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

fn display_js(value: Option<&Value>) -> String {
    match value {
        None => "undefined".to_string(),
        Some(Value::Null) => "null".to_string(),
        Some(Value::Bool(value)) => value.to_string(),
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::String(value)) => value.clone(),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| match value {
                Value::Null | Value::Array(_) | Value::Object(_) => String::new(),
                other => display_js(Some(other)),
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use serde_json::{json, Map};

    use super::*;

    const BASE_FILES: &[(&str, &str)] = &[
        ("schema/custom.tql", "define\ncustom sub entity;"),
        ("data/seed.tql", "insert $x isa thing;"),
        ("manifests/resources.json", "{}"),
    ];

    fn base_package() -> Value {
        json!({
            "name": "custom",
            "displayName": "custom",
            "version": "1.0.0",
            "scripts": {
                "refresh:package-contract": "node refresh.mjs",
                "validate:bootstrap": "node validate-bootstrap.mjs",
                "test:typedb-bootstrap": "node test-bootstrap.mjs"
            },
            "schemas": [{ "name": "custom", "file": "schema/custom.tql" }],
            "data": ["data/seed.tql"],
            "manifests": ["manifests/resources.json"],
            "provenance": {
                "manifest": "manifests/resources.json",
                "status": "bootstrap"
            },
            "assembly": {
                "loadOrder": ["schema/custom.tql", "data/seed.tql", "manifests/resources.json"]
            }
        })
    }

    fn write(root: &Path, relative_path: &str, contents: &str) {
        let path = root.join(relative_path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    fn run_git(root: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(root)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    fn create_fixture(package: &Value, extra_files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (relative_path, contents) in BASE_FILES.iter().chain(extra_files) {
            write(dir.path(), relative_path, contents);
        }
        let mut package_text = serde_json::to_string_pretty(package).unwrap();
        package_text.push('\n');
        write(dir.path(), "package.json", &package_text);
        write(dir.path(), ".gitignore", "node_modules/\n");
        run_git(dir.path(), &["init", "-b", "main"]);
        run_git(dir.path(), &["config", "user.name", "Fixture"]);
        run_git(dir.path(), &["config", "user.email", "fixture@example.com"]);
        run_git(dir.path(), &["add", "."]);
        run_git(dir.path(), &["commit", "-m", "Initial fixture"]);
        dir
    }

    fn object_mut(value: &mut Value) -> &mut Map<String, Value> {
        value.as_object_mut().unwrap()
    }

    fn assert_message(package: &Value, extra_files: &[(&str, &str)], expected: &str) {
        let dir = create_fixture(package, extra_files);
        match validate_package_contract(dir.path()) {
            Err(Error::Version(message)) => assert_eq!(message, expected),
            other => panic!("expected Version({expected:?}), got {other:?}"),
        }
    }

    #[test]
    fn accepts_a_valid_self_describing_package() {
        let dir = create_fixture(&base_package(), &[]);
        validate_package_contract(dir.path()).unwrap();
    }

    #[test]
    fn rejects_undeclared_assembly_assets() {
        let mut package = base_package();
        object_mut(&mut package).insert(
            "assembly".to_string(),
            json!({ "loadOrder": ["schema/custom.tql", "data/not-declared.tql"] }),
        );
        assert_message(
            &package,
            &[("data/not-declared.tql", "insert $x isa thing;")],
            "assembly.loadOrder references undeclared asset: data/not-declared.tql",
        );
    }

    #[test]
    fn rejects_monolithic_schema_docs_when_apply_units_exist() {
        let mut package = base_package();
        object_mut(&mut package).insert(
            "data".to_string(),
            json!([
                "data/seed.tql",
                "generated/apply-units/data/custom-schema-docs/0001.tql",
                "data/custom-schema-docs.tql"
            ]),
        );
        object_mut(&mut package).insert(
            "assembly".to_string(),
            json!({
                "loadOrder": [
                    "schema/custom.tql",
                    "data/seed.tql",
                    "generated/apply-units/data/custom-schema-docs/0001.tql",
                    "data/custom-schema-docs.tql",
                    "manifests/resources.json"
                ],
                "generatedArtifacts": ["data/custom-schema-docs.tql"]
            }),
        );
        assert_message(
            &package,
            &[
                (
                    "generated/apply-units/data/custom-schema-docs/0001.tql",
                    "insert $doc isa thing;",
                ),
                ("data/custom-schema-docs.tql", "insert $doc isa thing;"),
            ],
            "schema docs 'data/custom-schema-docs.tql' must not be listed as executable data when split apply units exist",
        );
    }

    #[test]
    fn allows_monolithic_schema_docs_as_generated_artifact() {
        let mut package = base_package();
        object_mut(&mut package).insert(
            "data".to_string(),
            json!([
                "data/seed.tql",
                "generated/apply-units/data/custom-schema-docs/0001.tql"
            ]),
        );
        object_mut(&mut package).insert(
            "assembly".to_string(),
            json!({
                "loadOrder": [
                    "schema/custom.tql",
                    "data/seed.tql",
                    "generated/apply-units/data/custom-schema-docs/0001.tql",
                    "manifests/resources.json"
                ],
                "generatedArtifacts": ["data/custom-schema-docs.tql"]
            }),
        );
        let dir = create_fixture(
            &package,
            &[
                (
                    "generated/apply-units/data/custom-schema-docs/0001.tql",
                    "insert $doc isa thing;",
                ),
                ("data/custom-schema-docs.tql", "insert $doc isa thing;"),
            ],
        );
        validate_package_contract(dir.path()).unwrap();
    }

    #[test]
    fn requires_live_migration_testing_when_migrations_are_declared() {
        let mut package = base_package();
        object_mut(&mut package).insert(
            "migration".to_string(),
            json!({
                "format": 1,
                "plans": [{
                    "id": "custom-0.9.x-to-1.0.0",
                    "from": "0.9.x",
                    "to": "1.0.0",
                    "mode": "compatible",
                    "phases": [{
                        "id": "write",
                        "units": [{ "kind": "write", "path": "migrations/v0.9.0-to-v1.0.0.tql" }]
                    }]
                }]
            }),
        );
        assert_message(
            &package,
            &[(
                "migrations/v0.9.0-to-v1.0.0.tql",
                "match $x isa thing; insert $y isa thing;",
            )],
            "package.json scripts must define 'test:typedb-migration'",
        );
    }

    fn migration_package(from: &str, migration_path: &str) -> Value {
        let mut package = base_package();
        object_mut(&mut package).insert("version".to_string(), json!("1.0.1"));
        package["scripts"]["test:typedb-migration"] = json!("node test-migration.mjs");
        object_mut(&mut package).insert(
            "migration".to_string(),
            json!({
                "format": 1,
                "plans": [{
                    "id": format!("custom-{from}-to-1.0.1"),
                    "from": from,
                    "to": "1.0.1",
                    "mode": "compatible",
                    "phases": [{
                        "id": "write",
                        "units": [{ "kind": "write", "path": migration_path }]
                    }]
                }]
            }),
        );
        package
    }

    #[test]
    fn rejects_plans_that_do_not_cover_the_previous_release_tag() {
        let package = migration_package("0.8.x", "migrations/v0.8.0-to-v1.0.1.tql");
        let dir = create_fixture(
            &package,
            &[(
                "migrations/v0.8.0-to-v1.0.1.tql",
                "match $x isa thing; insert $y isa thing;",
            )],
        );
        run_git(dir.path(), &["tag", "v1.0.0"]);
        match validate_package_contract(dir.path()) {
            Err(Error::Version(message)) => assert_eq!(
                message,
                "migration plans do not cover previous release 1.0.0 -> 1.0.1"
            ),
            other => panic!("expected previous-release coverage error, got {other:?}"),
        }
    }

    #[test]
    fn accepts_plans_that_cover_the_previous_release_tag() {
        let package = migration_package(">=1.0.0 <1.0.1", "migrations/v1.0.0-to-v1.0.1.tql");
        let dir = create_fixture(
            &package,
            &[(
                "migrations/v1.0.0-to-v1.0.1.tql",
                "match $x isa thing; insert $y isa thing;",
            )],
        );
        run_git(dir.path(), &["tag", "v1.0.0"]);
        validate_package_contract(dir.path()).unwrap();
    }
}
