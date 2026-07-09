//! Pure package-release planning with parity to the Node release tool.

use std::collections::HashSet;

use serde_json::{Map, Value};

use crate::error::{Error, Result};
use crate::version::{resolve_release_version, BumpKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleasePlan {
    pub current_version: String,
    pub next_version: String,
    pub next_package_json: Value,
    pub rename_plan: Vec<Rename>,
    pub resume_existing_version: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rename {
    pub from: String,
    pub to: String,
}

fn release_error(message: impl Into<String>) -> Error {
    Error::Version(message.into())
}

fn replace_version_token(value: &Value, current_version: &str, next_version: &str) -> Value {
    match value {
        Value::String(value) => Value::String(value.replace(current_version, next_version)),
        _ => value.clone(),
    }
}

fn dedupe_renames(rename_plan: Vec<Rename>) -> Vec<Rename> {
    let mut seen = HashSet::new();
    rename_plan
        .into_iter()
        .filter(|rename| seen.insert(format!("{}=>{}", rename.from, rename.to)))
        .collect()
}

fn parse_semver(value: &str) -> Result<(u64, u64, u64)> {
    let mut parts = value.split('.');
    let parsed = (|| {
        let major = parse_semver_component(parts.next()?)?;
        let minor = parse_semver_component(parts.next()?)?;
        let patch = parse_semver_component(parts.next()?)?;
        if parts.next().is_some() {
            return None;
        }
        Some((major, minor, patch))
    })();

    parsed.ok_or_else(|| release_error(format!("Invalid semver value: {value}")))
}

fn parse_semver_component(value: &str) -> Option<u64> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

fn semver_minor_range(version: &str) -> Result<String> {
    let (major, minor, _) = parse_semver(version)?;
    Ok(format!("{major}.{minor}.x"))
}

fn has_user_authored_phases(plan: &Value) -> bool {
    plan.get("phases")
        .and_then(Value::as_array)
        .is_some_and(|phases| {
            phases.iter().any(|phase| {
                phase
                    .get("units")
                    .and_then(Value::as_array)
                    .is_some_and(|units| {
                        units
                            .iter()
                            .any(|unit| unit.get("kind").and_then(Value::as_str) == Some("schema"))
                    })
            })
        })
}

fn rewrite_migration_metadata(
    next_package_json: &mut Value,
    current_version: &str,
    next_version: &str,
) -> Result<()> {
    let name = next_package_json
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("undefined")
        .to_string();
    let Some(migration) = next_package_json
        .get_mut("migration")
        .and_then(Value::as_object_mut)
    else {
        return Ok(());
    };

    migration.insert(
        "supportsUpgradeFrom".to_string(),
        Value::Array(vec![Value::String(semver_minor_range(current_version)?)]),
    );

    let Some(plans) = migration.get("plans").and_then(Value::as_array) else {
        return Ok(());
    };
    if plans.is_empty() {
        return Ok(());
    }

    let plan_count = plans.len();
    let plans = plans.clone();
    let default_id = format!("{name}-{current_version}-to-{next_version}");
    let rewritten_plans = plans
        .iter()
        .map(|plan| {
            let original = plan.as_object();
            let mut base = original.cloned().unwrap_or_default();
            let plan_id = if plan_count > 1 {
                original
                    .and_then(|plan| plan.get("id"))
                    .cloned()
                    .unwrap_or(Value::Null)
            } else {
                Value::String(default_id.clone())
            };
            let from = original
                .and_then(|plan| plan.get("from"))
                .cloned()
                .unwrap_or(Value::Null);
            let mut snapshot = original
                .and_then(|plan| plan.get("snapshot"))
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let required = if original
                .and_then(|plan| plan.get("mode"))
                .and_then(Value::as_str)
                == Some("replace")
            {
                Value::Bool(true)
            } else {
                original
                    .and_then(|plan| plan.get("snapshot"))
                    .and_then(Value::as_object)
                    .and_then(|snapshot| snapshot.get("required"))
                    .filter(|required| !required.is_null())
                    .cloned()
                    .unwrap_or(Value::Bool(false))
            };

            base.insert("id".to_string(), plan_id);
            base.insert("from".to_string(), from);
            base.insert("to".to_string(), Value::String(next_version.to_string()));
            snapshot.insert("required".to_string(), required);
            snapshot.insert(
                "label".to_string(),
                Value::String(format!("pre-{name}-{next_version}-migration")),
            );
            base.insert("snapshot".to_string(), Value::Object(snapshot));

            if !has_user_authored_phases(plan) {
                base.insert(
                    "from".to_string(),
                    Value::String(current_version.to_string()),
                );
                base.insert(
                    "phases".to_string(),
                    generated_phases(current_version, next_version),
                );
            }

            Value::Object(base)
        })
        .collect();

    migration.insert("plans".to_string(), Value::Array(rewritten_plans));
    Ok(())
}

fn generated_phases(current_version: &str, next_version: &str) -> Value {
    Value::Array(vec![
        phase(
            "preflight",
            "assert-data",
            format!("migrations/preflight/assert-v{current_version}-module-version.tql"),
        ),
        phase(
            "migrate",
            "write",
            format!("migrations/v{current_version}-to-v{next_version}.tql"),
        ),
        phase(
            "verify",
            "assert-data",
            format!("migrations/verify/assert-v{next_version}-module-version.tql"),
        ),
    ])
}

fn phase(id: &str, kind: &str, path: String) -> Value {
    let mut unit = Map::new();
    unit.insert("kind".to_string(), Value::String(kind.to_string()));
    unit.insert("path".to_string(), Value::String(path));

    let mut phase = Map::new();
    phase.insert("id".to_string(), Value::String(id.to_string()));
    phase.insert("units".to_string(), Value::Array(vec![Value::Object(unit)]));
    Value::Object(phase)
}

fn rewrite_path_array(
    container: &mut Map<String, Value>,
    key: &str,
    current_version: &str,
    next_version: &str,
    rename_plan: &mut Vec<Rename>,
) {
    let Some(entries) = container.get_mut(key).and_then(Value::as_array_mut) else {
        return;
    };

    for entry in entries {
        let rewritten = replace_version_token(entry, current_version, next_version);
        if rewritten != *entry {
            rename_plan.push(Rename {
                from: entry
                    .as_str()
                    .expect("only strings are rewritten")
                    .to_string(),
                to: rewritten
                    .as_str()
                    .expect("rewritten strings remain strings")
                    .to_string(),
            });
        }
        *entry = rewritten;
    }
}

pub fn plan_package_release(
    package_json: &Value,
    bump: Option<&str>,
    version: Option<&str>,
) -> Result<ReleasePlan> {
    let current_version = package_json
        .get("version")
        .and_then(Value::as_str)
        .filter(|version| !version.is_empty())
        .ok_or_else(|| release_error("Target package.json is missing version"))?
        .to_string();
    let bump = bump.map(str::parse::<BumpKind>).transpose()?;
    let next_version = resolve_release_version(&current_version, version, bump)?;
    let resume_existing_version = version.is_some() && next_version == current_version;

    if next_version == current_version && !resume_existing_version {
        return Err(release_error(format!(
            "Release version is unchanged: {current_version}"
        )));
    }

    if resume_existing_version {
        return Ok(ReleasePlan {
            current_version,
            next_version,
            next_package_json: package_json.clone(),
            rename_plan: Vec::new(),
            resume_existing_version: true,
        });
    }

    let mut next_package_json = package_json.clone();
    if let Some(package) = next_package_json.as_object_mut() {
        package.insert("version".to_string(), Value::String(next_version.clone()));
    }
    rewrite_migration_metadata(&mut next_package_json, &current_version, &next_version)?;

    let mut rename_plan = Vec::new();
    let Some(package) = next_package_json.as_object_mut() else {
        unreachable!("a package with a version field is an object");
    };

    rewrite_path_array(
        package,
        "manifests",
        &current_version,
        &next_version,
        &mut rename_plan,
    );

    if let Some(manifest) = package
        .get_mut("provenance")
        .and_then(Value::as_object_mut)
        .and_then(|provenance| provenance.get_mut("manifest"))
        .filter(|manifest| manifest.is_string())
    {
        let rewritten = replace_version_token(manifest, &current_version, &next_version);
        if rewritten != *manifest {
            rename_plan.push(Rename {
                from: manifest.as_str().expect("manifest is a string").to_string(),
                to: rewritten
                    .as_str()
                    .expect("rewritten manifest is a string")
                    .to_string(),
            });
            *manifest = rewritten;
        }
    }

    if let Some(assembly) = package.get_mut("assembly").and_then(Value::as_object_mut) {
        rewrite_path_array(
            assembly,
            "generatedArtifacts",
            &current_version,
            &next_version,
            &mut rename_plan,
        );
        rewrite_path_array(
            assembly,
            "loadOrder",
            &current_version,
            &next_version,
            &mut rename_plan,
        );
    }

    if let Some(scripts) = package.get_mut("scripts").and_then(Value::as_object_mut) {
        for script in scripts.values_mut() {
            *script = replace_version_token(script, &current_version, &next_version);
        }
    }

    Ok(ReleasePlan {
        current_version,
        next_version,
        next_package_json,
        rename_plan: dedupe_renames(rename_plan),
        resume_existing_version: false,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn plan_package_release_rewrites_versioned_manifest_references() {
        let plan = plan_package_release(
            &json!({
                "name": "example",
                "version": "1.0.0",
                "manifests": ["manifests/example-v1.0.0.package-manifest.json"],
                "provenance": {
                    "manifest": "manifests/example-v1.0.0.package-manifest.json",
                },
                "assembly": {
                    "loadOrder": [
                        "schema/example.tql",
                        "manifests/example-v1.0.0.package-manifest.json",
                    ],
                    "generatedArtifacts": [
                        "manifests/example-v1.0.0.package-manifest.json",
                        "manifests/example-v1.0.0.report.json",
                    ],
                },
                "upstream": {
                    "tag": "v1.0.0",
                },
                "scripts": {
                    "refresh:package-contract": "node tools/package_contract/refresh_package_contract.mjs",
                    "validate:bootstrap": "node tools/package_contract/validate_bootstrap.mjs",
                    "test:typedb-bootstrap": "node tools/package_contract/validate_typedb_bootstrap.mjs",
                },
            }),
            Some("patch"),
            None,
        )
        .unwrap();

        assert_eq!(plan.next_version, "1.0.1");
        assert_eq!(
            plan.next_package_json.pointer("/provenance/manifest"),
            Some(&json!("manifests/example-v1.0.1.package-manifest.json"))
        );
        assert_eq!(
            plan.next_package_json.pointer("/assembly/loadOrder"),
            Some(&json!([
                "schema/example.tql",
                "manifests/example-v1.0.1.package-manifest.json",
            ]))
        );
        assert_eq!(
            plan.rename_plan,
            vec![
                Rename {
                    from: "manifests/example-v1.0.0.package-manifest.json".to_string(),
                    to: "manifests/example-v1.0.1.package-manifest.json".to_string(),
                },
                Rename {
                    from: "manifests/example-v1.0.0.report.json".to_string(),
                    to: "manifests/example-v1.0.1.report.json".to_string(),
                },
            ]
        );
    }

    #[test]
    fn plan_package_release_preserves_upstream_metadata_and_rewrites_scripts() {
        let plan = plan_package_release(
            &json!({
                "name": "gist",
                "version": "14.0.0",
                "manifests": ["manifests/gist-v14.0.0.translation-manifest.json"],
                "provenance": {
                    "manifest": "manifests/gist-v14.0.0.translation-manifest.json",
                },
                "assembly": {
                    "generatedArtifacts": [
                        "manifests/gist-v14.0.0.translation-manifest.json",
                        "manifests/gist-v14.0.0.ir-summary.json",
                    ],
                },
                "upstream": {
                    "repo": "https://github.com/semanticarts/gist",
                    "tag": "v14.0.0",
                    "commit": "6ab80c158a7fa56a1b5d3d824b125b92107e8f08",
                },
                "scripts": {
                    "parse:ir": "node tools/gist_to_typeql/parse_gist.mjs --out manifests/gist-v14.0.0.ir-summary.json",
                    "refresh:package-contract": "npm run parse:ir && npm run emit:structural",
                    "validate:bootstrap": "node tools/gist_to_typeql/validate_bootstrap.mjs",
                    "test:typedb-bootstrap": "node tools/package_contract/validate_typedb_bootstrap.mjs",
                },
            }),
            None,
            Some("1.0.0"),
        )
        .unwrap();

        assert_eq!(plan.next_version, "1.0.0");
        assert_eq!(
            plan.next_package_json.pointer("/upstream/tag"),
            Some(&json!("v14.0.0"))
        );
        assert_eq!(
            plan.next_package_json.pointer("/upstream/repo"),
            Some(&json!("https://github.com/semanticarts/gist"))
        );
        assert_eq!(
            plan.next_package_json.pointer("/upstream/commit"),
            Some(&json!("6ab80c158a7fa56a1b5d3d824b125b92107e8f08"))
        );
        assert_eq!(
            plan.next_package_json.pointer("/scripts/parse:ir"),
            Some(&json!(
                "node tools/gist_to_typeql/parse_gist.mjs --out manifests/gist-v1.0.0.ir-summary.json"
            ))
        );
        assert_eq!(
            plan.next_package_json
                .pointer("/scripts/validate:bootstrap"),
            Some(&json!("node tools/gist_to_typeql/validate_bootstrap.mjs"))
        );
        assert_eq!(
            plan.next_package_json.pointer("/provenance/manifest"),
            Some(&json!("manifests/gist-v1.0.0.translation-manifest.json"))
        );
    }

    #[test]
    fn plan_package_release_rewrites_migration_metadata_for_the_next_release() {
        let plan = plan_package_release(
            &json!({
                "name": "vibemachine",
                "version": "0.6.0",
                "migration": {
                    "format": 1,
                    "supportsUpgradeFrom": ["0.5.x"],
                    "plans": [
                        {
                            "id": "vibemachine-0.5.0-to-0.6.0",
                            "from": "0.5.0",
                            "to": "0.6.0",
                            "mode": "replace",
                            "snapshot": {
                                "required": true,
                                "label": "pre-vibemachine-0.6.0-migration",
                            },
                            "phases": [
                                {
                                    "id": "preflight",
                                    "units": [{
                                        "kind": "assert-data",
                                        "path": "migrations/preflight/assert-v0.5.0-module-version.tql",
                                    }],
                                },
                                {
                                    "id": "migrate",
                                    "units": [{
                                        "kind": "write",
                                        "path": "migrations/v0.5.0-to-v0.6.0.tql",
                                    }],
                                },
                                {
                                    "id": "verify",
                                    "units": [{
                                        "kind": "assert-data",
                                        "path": "migrations/verify/assert-v0.6.0-module-version.tql",
                                    }],
                                },
                            ],
                        },
                    ],
                },
                "scripts": {
                    "refresh:package-contract": "node tools/package_contract/refresh_package_contract.mjs",
                    "validate:bootstrap": "node tools/package_contract/validate_bootstrap.mjs",
                    "test:typedb-bootstrap": "node tools/package_contract/validate_typedb_bootstrap.mjs",
                },
            }),
            Some("minor"),
            None,
        )
        .unwrap();

        assert_eq!(plan.next_version, "0.7.0");
        assert_eq!(
            plan.next_package_json
                .pointer("/migration/supportsUpgradeFrom"),
            Some(&json!(["0.6.x"]))
        );
        assert_eq!(
            plan.next_package_json.pointer("/migration/plans/0/id"),
            Some(&json!("vibemachine-0.6.0-to-0.7.0"))
        );
        assert_eq!(
            plan.next_package_json.pointer("/migration/plans/0/from"),
            Some(&json!("0.6.0"))
        );
        assert_eq!(
            plan.next_package_json.pointer("/migration/plans/0/to"),
            Some(&json!("0.7.0"))
        );
        assert_eq!(
            plan.next_package_json
                .pointer("/migration/plans/0/snapshot/label"),
            Some(&json!("pre-vibemachine-0.7.0-migration"))
        );
        assert_eq!(
            plan.next_package_json.pointer("/migration/plans/0/phases"),
            Some(&json!([
                {
                    "id": "preflight",
                    "units": [{
                        "kind": "assert-data",
                        "path": "migrations/preflight/assert-v0.6.0-module-version.tql",
                    }],
                },
                {
                    "id": "migrate",
                    "units": [{
                        "kind": "write",
                        "path": "migrations/v0.6.0-to-v0.7.0.tql",
                    }],
                },
                {
                    "id": "verify",
                    "units": [{
                        "kind": "assert-data",
                        "path": "migrations/verify/assert-v0.7.0-module-version.tql",
                    }],
                },
            ]))
        );
    }

    #[test]
    fn plan_package_release_supports_resuming_an_existing_explicit_version() {
        let package_json = json!({
            "name": "gist",
            "version": "1.0.3",
            "manifests": ["manifests/gist-v1.0.3.translation-manifest.json"],
            "provenance": {
                "manifest": "manifests/gist-v1.0.3.translation-manifest.json",
            },
            "assembly": {
                "generatedArtifacts": ["manifests/gist-v1.0.3.translation-manifest.json"],
            },
            "scripts": {
                "refresh:package-contract": "node tools/package_contract/refresh_package_contract.mjs",
                "validate:bootstrap": "node tools/package_contract/validate_bootstrap.mjs",
                "test:typedb-bootstrap": "node tools/package_contract/validate_typedb_bootstrap.mjs",
            },
        });

        let plan = plan_package_release(&package_json, None, Some("1.0.3")).unwrap();

        assert_eq!(plan.current_version, "1.0.3");
        assert_eq!(plan.next_version, "1.0.3");
        assert!(plan.resume_existing_version);
        assert!(plan.rename_plan.is_empty());
        assert_eq!(plan.next_package_json, package_json);
    }
}
