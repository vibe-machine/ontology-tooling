//! `ont check-upgrade`: runs a package's declared upgrade plans the way the
//! app does, on a throwaway TypeDB whose database holds more than one earlier
//! release.
//!
//! Faults such as a `match` that selects every version's record only show
//! when several releases' records coexist, so the base database is the
//! package's dependencies (sibling `ontology-<name>` working trees), then the
//! release before the plan's source release, then the source release itself
//! installed over it, as the app installs an update (schemas, then every data
//! file, one transaction each). The plan then runs in the app's order (see
//! [`check_upgrade`]), each unit in its own transaction and under a time
//! limit, followed by what the app does after any upgrade.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use futures::StreamExt;
use serde::Serialize;
use serde_json::Value;
use typedb_driver::answer::QueryAnswer;
use typedb_driver::{
    Addresses, Credentials, DriverOptions, DriverTlsConfig, TransactionType, TypeDBDriver,
};

use crate::apply::{apply_schema, create_database, TypeDbTarget};
use crate::competency_questions::resolve_package_order;
use crate::error::{Error, Result};
use crate::upgrade_limits::split_functions;
use crate::version::{SemanticVersion, VersionRange};

fn invalid<S: Into<String>>(message: S) -> Error {
    Error::Version(message.into())
}

/// How one case ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "status", content = "detail")]
pub enum CaseStatus {
    /// The upgrade completed and the candidate checks hold.
    Passed,
    /// The base release could not be installed on today's TypeDB and
    /// dependencies; nothing was upgraded from it.
    UnusableBase(String),
    /// The candidate declares (by leaving it out of `supportsUpgradeFrom`)
    /// that it does not upgrade from this release, and the app refuses, as
    /// declared.
    NotSupported(String),
    /// The upgrade from an installed base failed.
    Failed(String),
}

/// One upgrade case: from one released tag, or through all of history.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaseOutcome {
    pub name: String,
    /// The path the app takes for the final upgrade: `plan <id>`,
    /// `upgrade file …`, or `re-apply data files`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Notes on the history replayed before the final upgrade.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    pub millis: u128,
    #[serde(flatten)]
    pub status: CaseStatus,
}

/// Which cases to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckScope {
    /// Every released tag on its own, plus the lived-through database.
    Full,
    /// The previous tag on its own, plus the lived-through database.
    Quick,
}

/// Where a file's text comes from: the working tree or a released tag.
#[derive(Clone, Copy)]
pub enum Source<'a> {
    WorkingTree,
    Tag(&'a str),
}

pub fn read_file(root: &Path, source: Source<'_>, relative_path: &str) -> Result<String> {
    match source {
        Source::WorkingTree => std::fs::read_to_string(root.join(relative_path)).map_err(|error| {
            invalid(format!(
                "failed to read {}: {error}",
                root.join(relative_path).display()
            ))
        }),
        Source::Tag(tag) => {
            let output = Command::new("git")
                .args(["show", &format!("{tag}:{relative_path}")])
                .current_dir(root)
                .output()
                .map_err(|error| invalid(format!("git show failed: {error}")))?;
            if !output.status.success() {
                return Err(invalid(format!(
                    "{relative_path} is not in {tag} of {}",
                    root.display()
                )));
            }
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        }
    }
}

pub fn released_tags(repo_path: &Path) -> Result<Vec<(String, SemanticVersion)>> {
    let output = Command::new("git")
        .args(["tag", "--list", "v*.*.*"])
        .current_dir(repo_path)
        .output()
        .map_err(|error| invalid(format!("git tag failed: {error}")))?;
    let mut tags: Vec<(String, SemanticVersion)> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|tag| {
            SemanticVersion::parse(tag.trim().trim_start_matches('v'))
                .ok()
                .map(|version| (tag.trim().to_string(), version))
        })
        .collect();
    tags.sort_by_key(|tag| tag.1);
    Ok(tags)
}

async fn connect(target: &TypeDbTarget) -> Result<TypeDBDriver> {
    let addresses = Addresses::try_from_address_str(&target.address)
        .map_err(|error| Error::TypeDb(error.to_string()))?;
    TypeDBDriver::new(
        addresses,
        Credentials::new(&target.username, &target.password),
        DriverOptions::new(DriverTlsConfig::disabled()),
    )
    .await
    .map_err(|error| Error::TypeDb(error.to_string()))
}

/// Counts the rows a read query returns.
pub async fn count_rows(target: &TypeDbTarget, database: &str, query: &str) -> Result<usize> {
    let driver = connect(target).await?;
    let transaction = driver
        .transaction(database, TransactionType::Read)
        .await
        .map_err(|error| Error::TypeDb(error.to_string()))?;
    let answer = transaction
        .query(query)
        .await
        .map_err(|error| Error::TypeDb(error.to_string()))?;
    let mut rows = 0;
    match answer {
        QueryAnswer::Ok(_) => {}
        QueryAnswer::ConceptRowStream(_, mut stream) => {
            while let Some(row) = stream.next().await {
                row.map_err(|error| Error::TypeDb(error.to_string()))?;
                rows += 1;
            }
        }
        QueryAnswer::ConceptDocumentStream(_, mut stream) => {
            while let Some(document) = stream.next().await {
                document.map_err(|error| Error::TypeDb(error.to_string()))?;
                rows += 1;
            }
        }
    }
    Ok(rows)
}

/// Runs one write query in its own transaction, as the app runs a unit,
/// failing when it does not finish within `limit`.
pub async fn write_unit(
    target: &TypeDbTarget,
    database: &str,
    label: &str,
    query: &str,
    limit: Duration,
) -> Result<()> {
    let run = async {
        let driver = connect(target).await?;
        let transaction = driver
            .transaction(database, TransactionType::Write)
            .await
            .map_err(|error| Error::TypeDb(error.to_string()))?;
        transaction
            .query(query)
            .await
            .map_err(|error| Error::TypeDb(error.to_string()))?;
        transaction
            .commit()
            .await
            .map_err(|error| Error::TypeDb(error.to_string()))
    };
    match tokio::time::timeout(limit, run).await {
        Ok(result) => result.map_err(|error| invalid(format!("{label} failed: {error}"))),
        Err(_) => Err(invalid(format!(
            "{label} did not finish within {}s; a unit that chains independent matches \
             multiplies its rows and never finishes",
            limit.as_secs()
        ))),
    }
}

/// Applies schema over an installed database as the app does: the types in
/// one schema transaction, then each function redefined (or defined when the
/// database lacks it), since TypeDB refuses to `define` an installed function.
pub async fn apply_schema_over_installed(
    target: &TypeDbTarget,
    database: &str,
    blobs: &[String],
) -> Result<()> {
    let mut types = Vec::new();
    let mut functions = Vec::new();
    for blob in blobs {
        let (without, found) = split_functions(blob);
        let significant = without
            .lines()
            .map(|line| line.split('#').next().unwrap_or("").trim())
            .any(|line| !line.is_empty() && line != "define");
        if significant {
            types.push(without);
        }
        functions.extend(found);
    }
    if !types.is_empty() {
        apply_schema(target, database, &types).await?;
    }
    for function in functions {
        let redefined = apply_schema(target, database, &[format!("redefine\n{function}")]).await;
        if let Err(error) = redefined {
            if error.to_string().contains("FUN1") {
                apply_schema(target, database, &[format!("define\n{function}")]).await?;
            } else {
                return Err(error);
            }
        }
    }
    Ok(())
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect()
}

/// A package's schema files and data files, in load order.
pub fn assets(package: &Value) -> (Vec<String>, Vec<String>) {
    let schema_files: Vec<String> = package
        .get("schemas")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|schema| schema.get("file").and_then(Value::as_str))
        .map(ToOwned::to_owned)
        .collect();
    let mut load_order = string_list(package.pointer("/assembly/loadOrder"));
    if load_order.is_empty() {
        load_order = schema_files.clone();
        load_order.extend(string_list(package.get("data")));
    }
    let mut schemas = Vec::new();
    let mut data = Vec::new();
    for path in load_order.into_iter().filter(|path| path.ends_with(".tql")) {
        if schema_files.contains(&path) {
            schemas.push(path);
        } else {
            data.push(path);
        }
    }
    (schemas, data)
}

/// Installs a package version: schemas (over what is installed), then every
/// data file in its own transaction.
pub async fn install(
    target: &TypeDbTarget,
    database: &str,
    root: &Path,
    source: Source<'_>,
    label: &str,
    limit: Duration,
) -> Result<()> {
    let package: Value = serde_json::from_str(&read_file(root, source, "package.json")?)
        .map_err(|error| invalid(format!("package.json of {label}: {error}")))?;
    let (schemas, data) = assets(&package);
    let blobs = schemas
        .iter()
        .map(|path| read_file(root, source, path))
        .collect::<Result<Vec<_>>>()?;
    apply_schema_over_installed(target, database, &blobs)
        .await
        .map_err(|error| invalid(format!("schema of {label}: {error}")))?;
    for path in data {
        let text = read_file(root, source, &path)?;
        if text.trim().is_empty() {
            continue;
        }
        write_unit(target, database, &format!("{label} {path}"), &text, limit).await?;
    }
    Ok(())
}

async fn remove_schema_documentation(
    target: &TypeDbTarget,
    database: &str,
    name: &str,
    limit: Duration,
) {
    for (relation, roles) in [
        ("resourceInGroup", vec!["resource"]),
        (
            "schemaSubTypeOf",
            vec!["subtypeResource", "supertypeResource"],
        ),
        ("schemaOwns", vec!["ownerResource", "attributeResource"]),
        ("schemaPlays", vec!["playerResource", "roleResource"]),
        ("schemaRelates", vec!["relationResource", "roleResource"]),
    ] {
        let alternatives = roles
            .iter()
            .map(|role| format!("{{ $related links ({role}: $resource); }}"))
            .collect::<Vec<_>>()
            .join(" or ");
        let query = format!(
            "match\n  $module isa SchemaModule, has moduleKey \"{name}\";\n  (resource: $resource, \
             module: $module) isa inModule;\n  $related isa {relation};\n  {alternatives};\ndelete \
             $related;"
        );
        // A package without schema documentation types has nothing to delete.
        let _ = write_unit(
            target,
            database,
            "remove schema documentation",
            &query,
            limit,
        )
        .await;
    }
    let query = format!(
        "match\n  $module isa SchemaModule, has moduleKey \"{name}\";\n  $membership isa inModule, \
         links (resource: $resource, module: $module);\ndelete\n  $membership;\n  $resource;"
    );
    let _ = write_unit(
        target,
        database,
        "remove schema documentation",
        &query,
        limit,
    )
    .await;
}

fn is_rebuilt_after_upgrade(path: &str, text: &str) -> bool {
    path.contains("schema-docs")
        || text.contains("isa SchemaResource")
        || path
            .split('/')
            .any(|component| component.to_lowercase().contains("provenance"))
}

/// The app's single-file upgrade for an installed version
/// (`PackageBootstrapPlanner.migrationDiff`): `migrations/v<installed>-to-v<target>.tql`,
/// or else the shortest chain of such files from the installed version.
fn upgrade_files(
    root: &Path,
    source: Source<'_>,
    installed: &SemanticVersion,
    target_version: &SemanticVersion,
) -> Result<Option<Vec<String>>> {
    let names: Vec<String> = match source {
        Source::WorkingTree => std::fs::read_dir(root.join("migrations"))
            .map(|entries| {
                entries
                    .filter_map(std::result::Result::ok)
                    .filter(|entry| entry.path().is_file())
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default(),
        Source::Tag(tag) => {
            let output = Command::new("git")
                .args(["ls-tree", "--name-only", &format!("{tag}:migrations")])
                .current_dir(root)
                .output()
                .map_err(|error| invalid(format!("git ls-tree failed: {error}")))?;
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(ToOwned::to_owned)
                .collect()
        }
    };
    let mut edges: Vec<(SemanticVersion, SemanticVersion, String)> = names
        .iter()
        .filter_map(|name| {
            let stem = name.strip_suffix(".tql")?.strip_prefix('v')?;
            let (from, to) = stem.split_once("-to-v")?;
            Some((
                SemanticVersion::parse(from).ok()?,
                SemanticVersion::parse(to).ok()?,
                format!("migrations/{name}"),
            ))
        })
        .collect();
    edges.sort_by(|left, right| left.2.cmp(&right.2));
    if let Some((_, _, path)) = edges
        .iter()
        .find(|(from, to, _)| from == installed && to == target_version)
    {
        return Ok(Some(vec![path.clone()]));
    }
    // Breadth-first chain of at least two files, as the app searches.
    let mut queue: Vec<(SemanticVersion, Vec<String>)> = vec![(*installed, Vec::new())];
    let mut visited = vec![*installed];
    while !queue.is_empty() {
        let (at, path) = queue.remove(0);
        for (from, to, file) in &edges {
            if *from != at || visited.contains(to) {
                continue;
            }
            let mut next = path.clone();
            next.push(file.clone());
            if to == target_version {
                return Ok((next.len() >= 2).then_some(next));
            }
            visited.push(*to);
            queue.push((*to, next));
        }
    }
    Ok(None)
}

/// Upgrades the package installed at `installed` to the release in `source`
/// the way the app does (`OntologyViewModel+TypeDB.swift`
/// `crateBootstrapExecutions` and `applyPackageBootstrapExecution`), and
/// returns the path it took:
///
/// - a `migration` plan covering `installed`: its phases in order (assertions
///   must return rows; a `schema` unit in one schema transaction; each `write`
///   unit in its own transaction), then the package schemas, then schema
///   documentation deleted and re-applied with the provenance files;
/// - else an upgrade file or chain: the package schemas, each file as one
///   query, then documentation and provenance as above;
/// - else the app refuses the upgrade (`migrationPlan(forSchemaApplication:)`
///   asks `PackageMigrationPlanner.makePlan`, which throws "no migration plan
///   matches installed version"): an error starting [`REFUSED`].
///
/// (The app re-applies every data file only when it cannot read the installed
/// version from the database's provenance, which installs the release as if
/// fresh, or when its knowledge-bootstrap path reconciles packages; neither
/// is an upgrade an author can rely on.)
pub async fn upgrade_as_the_app_does(
    target: &TypeDbTarget,
    database: &str,
    root: &Path,
    source: Source<'_>,
    installed: &SemanticVersion,
    limit: Duration,
) -> Result<String> {
    let package: Value = serde_json::from_str(&read_file(root, source, "package.json")?)
        .map_err(|error| invalid(format!("package.json: {error}")))?;
    let version_text = package
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let version = SemanticVersion::parse(version_text)?;
    let name = package
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let (schemas, data) = assets(&package);
    let blobs = schemas
        .iter()
        .map(|path| read_file(root, source, path))
        .collect::<Result<Vec<_>>>()?;

    let plan = string_plans(&package).into_iter().find(|plan| {
        plan.get("to").and_then(Value::as_str) == Some(version_text)
            && plan
                .get("from")
                .and_then(Value::as_str)
                .and_then(|range| VersionRange::parse(range).ok())
                .is_some_and(|range| range.satisfies(installed))
    });

    let taken;
    if let Some(plan) = plan {
        let plan_id = plan
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("plan")
            .to_string();
        for phase in plan
            .get("phases")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            for unit in phase
                .get("units")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let kind = unit.get("kind").and_then(Value::as_str).unwrap_or("");
                let path = unit.get("path").and_then(Value::as_str).unwrap_or("");
                let text = read_file(root, source, path)?;
                let label = format!("plan '{plan_id}' unit {path}");
                match kind {
                    "assert-data" | "assert-schema" => {
                        if count_rows(target, database, &text).await? == 0 {
                            return Err(invalid(format!("{label}: assertion returned no rows")));
                        }
                    }
                    "schema" => apply_schema(target, database, &[text])
                        .await
                        .map_err(|error| invalid(format!("{label} failed: {error}")))?,
                    _ => write_unit(target, database, &label, &text, limit).await?,
                }
            }
        }
        apply_schema_over_installed(target, database, &blobs).await?;
        taken = format!("plan {plan_id}");
    } else if let Some(files) = upgrade_files(root, source, installed, &version)? {
        apply_schema_over_installed(target, database, &blobs).await?;
        for file in &files {
            let text = read_file(root, source, file)?;
            write_unit(
                target,
                database,
                &format!("upgrade file {file}"),
                &text,
                limit,
            )
            .await?;
        }
        taken = format!("upgrade file {}", files.join(" + "));
    } else {
        return Err(invalid(format!(
            "{REFUSED}: no migration plan matches installed version {installed} -> {version}, \
             and there is no upgrade file for it"
        )));
    }

    remove_schema_documentation(target, database, &name, limit).await;
    for path in data {
        let text = read_file(root, source, &path)?;
        if text.trim().is_empty() || !is_rebuilt_after_upgrade(&path, &text) {
            continue;
        }
        write_unit(target, database, &format!("re-apply {path}"), &text, limit).await?;
    }
    Ok(taken)
}

/// How an upgrade the app refuses is reported.
pub const REFUSED: &str = "the app refuses this upgrade";

/// The query the app uses to read a package's installed versions.
pub fn module_version_query(repo_url: &str, version: &str) -> String {
    format!(
        "match\n  $module isa OntologyModule, has moduleRepoUrl \"{repo_url}\";\n  $v isa \
         OntologyModuleVersion, has moduleVersion \"{version}\";\n  (version: $v, module: $module) \
         isa ontologyModuleVersionOf;"
    )
}

struct Candidate<'a> {
    repo_path: &'a Path,
    package: Value,
    version_text: String,
    repo_url: String,
    dependencies: Vec<PathBuf>,
}

impl Candidate<'_> {
    /// Whether the candidate's `supportsUpgradeFrom` covers `version`.
    fn supports(&self, version: &SemanticVersion) -> bool {
        let next = SemanticVersion {
            major: u64::MAX,
            minor: 0,
            patch: 0,
        };
        !crate::package_release::supported_sources(&self.package, &[*version], &next).is_empty()
    }

    /// Whether one of the candidate's plans upgrades from `version`.
    fn has_plan_from(&self, version: &SemanticVersion) -> bool {
        string_plans(&self.package).iter().any(|plan| {
            plan.get("to").and_then(Value::as_str) == Some(self.version_text.as_str())
                && plan
                    .get("from")
                    .and_then(Value::as_str)
                    .and_then(|range| VersionRange::parse(range).ok())
                    .is_some_and(|range| range.satisfies(version))
        })
    }
}

async fn fresh_database(
    target: &TypeDbTarget,
    database: &str,
    candidate: &Candidate<'_>,
    limit: Duration,
) -> Result<()> {
    create_database(target, database).await?;
    for dependency in &candidate.dependencies {
        install(
            target,
            database,
            dependency,
            Source::WorkingTree,
            &format!("dependency {}", dependency.display()),
            limit,
        )
        .await?;
    }
    Ok(())
}

/// The candidate's version is recorded and, for a package that installs its
/// competency questions, every question is a `StoredQuestion` with an answer.
async fn check_candidate(
    target: &TypeDbTarget,
    database: &str,
    candidate: &Candidate<'_>,
) -> Result<()> {
    let version = &candidate.version_text;
    if count_rows(
        target,
        database,
        &module_version_query(&candidate.repo_url, version),
    )
    .await?
        == 0
    {
        return Err(invalid(format!(
            "module version {version} is not recorded for {}",
            candidate.repo_url
        )));
    }
    if let Some((expected, present, answered)) =
        stored_question_counts(target, database, candidate.repo_path, &candidate.package).await?
    {
        if present != expected || answered != expected {
            return Err(invalid(format!(
                "{present} StoredQuestion records for this package ({answered} with an answer); \
                 the questions file has {expected}"
            )));
        }
    }
    Ok(())
}

/// Upgrades to the candidate at `repo_path` (the working tree) from every
/// released version, on one TypeDB server with a fresh database per case:
///
/// - from each released tag older than the candidate on its own (with
///   [`CheckScope::Quick`], only the newest): the dependencies, that tag, then
///   the upgrade as the app would run it for that installed version;
/// - one database that lived through history: the oldest tag that installs,
///   each later tag upgraded in order as the app would, then the candidate.
///   Several versions' records then sit side by side, which is when a match
///   that is not selected by a key multiplies.
///
/// After each, [`check_candidate`] must hold. A tag that cannot be installed
/// on today's TypeDB and dependencies is reported as an unusable base, not
/// skipped silently.
pub async fn check_upgrade(
    target: &TypeDbTarget,
    repo_path: &Path,
    scope: CheckScope,
    limit: Duration,
    progress: &mut dyn FnMut(&str),
) -> Result<Vec<CaseOutcome>> {
    let package: Value =
        serde_json::from_str(&read_file(repo_path, Source::WorkingTree, "package.json")?)
            .map_err(|error| invalid(format!("package.json: {error}")))?;
    let version_text = package
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let version = SemanticVersion::parse(&version_text)?;
    let ordered = resolve_package_order(repo_path)?;
    let candidate = Candidate {
        repo_path,
        repo_url: package
            .get("source")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        version_text,
        dependencies: ordered
            .iter()
            .take(ordered.len().saturating_sub(1))
            .map(|dependency| dependency.root.clone())
            .collect(),
        package,
    };
    let tags: Vec<(String, SemanticVersion)> = released_tags(repo_path)?
        .into_iter()
        .filter(|(_, tag_version)| *tag_version < version)
        .collect();
    let mut outcomes = Vec::new();
    let mut report = |outcome: CaseOutcome, outcomes: &mut Vec<CaseOutcome>| {
        let status = match &outcome.status {
            CaseStatus::Passed => "ok".to_string(),
            CaseStatus::UnusableBase(error) => format!("could not be used as a base: {error}"),
            CaseStatus::NotSupported(reason) => format!("not supported, as declared: {reason}"),
            CaseStatus::Failed(error) => format!("FAILED: {error}"),
        };
        let path = outcome
            .path
            .as_deref()
            .map(|path| format!(" via {path}"))
            .unwrap_or_default();
        progress(&format!(
            "{}{path}: {status} ({:.1}s)",
            outcome.name,
            outcome.millis as f64 / 1000.0
        ));
        outcomes.push(outcome);
    };

    let single_bases: Vec<&(String, SemanticVersion)> = match scope {
        CheckScope::Full => tags.iter().collect(),
        CheckScope::Quick => tags.last().into_iter().collect(),
    };
    for (index, (tag, tag_version)) in single_bases.into_iter().enumerate() {
        let started = Instant::now();
        let database = format!("upgrade_from_{index}");
        let name = format!("from {tag}");
        // Declared unsupported, with no plan or upgrade file for it: the app
        // refuses without touching the database, so there is nothing to run.
        if !candidate.supports(tag_version)
            && !candidate.has_plan_from(tag_version)
            && upgrade_files(repo_path, Source::WorkingTree, tag_version, &version)?.is_none()
        {
            report(
                CaseOutcome {
                    name,
                    path: None,
                    notes: Vec::new(),
                    millis: started.elapsed().as_millis(),
                    status: CaseStatus::NotSupported(format!(
                        "{REFUSED}: no migration plan matches installed version {tag_version} -> \
                         {version}, and there is no upgrade file for it"
                    )),
                },
                &mut outcomes,
            );
            continue;
        }
        let installed = async {
            fresh_database(target, &database, &candidate, limit).await?;
            install(target, &database, repo_path, Source::Tag(tag), tag, limit).await
        }
        .await;
        let (path, status) = match installed {
            Err(error) => (
                None,
                CaseStatus::UnusableBase(first_line(&error.to_string())),
            ),
            Ok(()) => {
                let upgraded = async {
                    let path = upgrade_as_the_app_does(
                        target,
                        &database,
                        repo_path,
                        Source::WorkingTree,
                        tag_version,
                        limit,
                    )
                    .await?;
                    check_candidate(target, &database, &candidate).await?;
                    Ok::<_, Error>(path)
                }
                .await;
                match upgraded {
                    Ok(path) => (Some(path), CaseStatus::Passed),
                    Err(error)
                        if error.to_string().contains(REFUSED)
                            && !candidate.supports(tag_version) =>
                    {
                        (
                            None,
                            CaseStatus::NotSupported(first_line(&error.to_string())),
                        )
                    }
                    Err(error) => (None, CaseStatus::Failed(error.to_string())),
                }
            }
        };
        report(
            CaseOutcome {
                name,
                path,
                notes: Vec::new(),
                millis: started.elapsed().as_millis(),
                status,
            },
            &mut outcomes,
        );
    }

    // The previous release after an earlier attempt failed part-way: the
    // candidate's schemas are already applied (as on a database whose upgrade
    // stopped after its schema step), so the plan must be safe to repeat.
    if let Some((tag, tag_version)) = tags.last() {
        let started = Instant::now();
        let database = "upgrade_after_interrupted_attempt".to_string();
        let blobs = assets(&candidate.package)
            .0
            .iter()
            .map(|path| read_file(repo_path, Source::WorkingTree, path))
            .collect::<Result<Vec<_>>>()?;
        let installed = async {
            fresh_database(target, &database, &candidate, limit).await?;
            install(target, &database, repo_path, Source::Tag(tag), tag, limit).await
        }
        .await;
        let (path, status) = match installed {
            Err(error) => (
                None,
                CaseStatus::UnusableBase(first_line(&error.to_string())),
            ),
            Ok(()) => {
                let upgraded = async {
                    apply_schema_over_installed(target, &database, &blobs).await?;
                    let path = upgrade_as_the_app_does(
                        target,
                        &database,
                        repo_path,
                        Source::WorkingTree,
                        tag_version,
                        limit,
                    )
                    .await?;
                    check_candidate(target, &database, &candidate).await?;
                    Ok::<_, Error>(path)
                }
                .await;
                match upgraded {
                    Ok(path) => (Some(path), CaseStatus::Passed),
                    Err(error) => (None, CaseStatus::Failed(error.to_string())),
                }
            }
        };
        report(
            CaseOutcome {
                name: format!("from {tag} after an interrupted attempt"),
                path,
                notes: Vec::new(),
                millis: started.elapsed().as_millis(),
                status,
            },
            &mut outcomes,
        );
    }

    // The database that lived through history.
    if !tags.is_empty() {
        let started = Instant::now();
        let database = "upgrade_through_history".to_string();
        let mut notes = Vec::new();
        let mut installed: Option<SemanticVersion> = None;
        let mut status = None;
        if let Err(error) = fresh_database(target, &database, &candidate, limit).await {
            status = Some(CaseStatus::Failed(format!("dependencies: {error}")));
        }
        for (tag, tag_version) in &tags {
            if status.is_some() {
                break;
            }
            match &installed {
                None => match install(target, &database, repo_path, Source::Tag(tag), tag, limit)
                    .await
                {
                    Ok(()) => {
                        notes.push(format!("installed {tag}"));
                        installed = Some(*tag_version);
                    }
                    Err(error) => {
                        notes.push(format!(
                            "{tag} could not be used as a base: {}",
                            first_line(&error.to_string())
                        ));
                        // A failed install can leave part of it behind: start again.
                        let _ = crate::apply::delete_database(target, &database).await;
                        if let Err(error) =
                            fresh_database(target, &database, &candidate, limit).await
                        {
                            status = Some(CaseStatus::Failed(format!("dependencies: {error}")));
                        }
                    }
                },
                Some(from) => {
                    match upgrade_as_the_app_does(
                        target,
                        &database,
                        repo_path,
                        Source::Tag(tag),
                        from,
                        limit,
                    )
                    .await
                    {
                        Ok(path) => notes.push(format!("{tag} via {path}")),
                        Err(error) => {
                            let reason = first_line(&error.to_string());
                            // Keep the older records, but make the database
                            // really hold this release before going on.
                            match install(target, &database, repo_path, Source::Tag(tag), tag, limit).await {
                                Ok(()) => notes.push(format!(
                                    "historical upgrade to {tag} failed ({reason}); installed {tag} over the database instead"
                                )),
                                Err(install_error) => {
                                    notes.push(format!(
                                        "historical upgrade to {tag} failed ({reason}) and so did installing it over the database ({}); started again from {tag}",
                                        first_line(&install_error.to_string())
                                    ));
                                    let _ = crate::apply::delete_database(target, &database).await;
                                    let restarted = async {
                                        fresh_database(target, &database, &candidate, limit).await?;
                                        install(target, &database, repo_path, Source::Tag(tag), tag, limit).await
                                    }
                                    .await;
                                    if let Err(error) = restarted {
                                        notes.push(format!("{tag} could not be used as a base: {}", first_line(&error.to_string())));
                                        installed = None;
                                        let _ = crate::apply::delete_database(target, &database).await;
                                        if let Err(error) = fresh_database(target, &database, &candidate, limit).await {
                                            status = Some(CaseStatus::Failed(format!("dependencies: {error}")));
                                        }
                                        continue;
                                    }
                                }
                            }
                        }
                    }
                    installed = Some(*tag_version);
                }
            }
        }
        let mut path = None;
        let status = match (status, installed) {
            (Some(status), _) => status,
            (None, None) => {
                CaseStatus::UnusableBase("no released tag could be installed".to_string())
            }
            (None, Some(from)) => {
                let upgraded = async {
                    let taken = upgrade_as_the_app_does(
                        target,
                        &database,
                        repo_path,
                        Source::WorkingTree,
                        &from,
                        limit,
                    )
                    .await?;
                    check_candidate(target, &database, &candidate).await?;
                    Ok::<_, Error>(taken)
                }
                .await;
                match upgraded {
                    Ok(taken) => {
                        path = Some(taken);
                        CaseStatus::Passed
                    }
                    Err(error) => CaseStatus::Failed(error.to_string()),
                }
            }
        };
        report(
            CaseOutcome {
                name: "through every release".to_string(),
                path,
                notes,
                millis: started.elapsed().as_millis(),
                status,
            },
            &mut outcomes,
        );
    }
    Ok(outcomes)
}

/// The first two non-empty lines of an error: the failing step and TypeDB's
/// own first reason.
fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(2)
        .collect::<Vec<_>>()
        .join(" ")
}

fn string_plans(package: &Value) -> Vec<Value> {
    package
        .pointer("/migration/plans")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// `(questions in the file, StoredQuestion records, those with an answer)`
/// for a package that installs its competency questions.
pub async fn stored_question_counts(
    target: &TypeDbTarget,
    database: &str,
    repo_path: &Path,
    package: &Value,
) -> Result<Option<(usize, usize, usize)>> {
    let Some(questions_path) = package.get("competencyQuestions").and_then(Value::as_str) else {
        return Ok(None);
    };
    let installs_questions = string_list(package.get("data"))
        .iter()
        .any(|path| path.ends_with("-questions.tql"));
    if !installs_questions {
        return Ok(None);
    }
    let file: Value =
        serde_json::from_str(&read_file(repo_path, Source::WorkingTree, questions_path)?)
            .map_err(|error| invalid(format!("{questions_path}: {error}")))?;
    let expected = file
        .get("questions")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let name = package.get("name").and_then(Value::as_str).unwrap_or("");
    let present = count_rows(
        target,
        database,
        &format!("match $q isa StoredQuestion, has storedPackage \"{name}\";"),
    )
    .await?;
    let answered = count_rows(
        target,
        database,
        &format!(
            "match $q isa StoredQuestion, has storedPackage \"{name}\";\n  storedAnswer (question: \
             $q, query: $x);\nselect $q;\ndistinct;"
        ),
    )
    .await?;
    Ok(Some((expected, present, answered)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assets_split_schemas_from_data_in_load_order() {
        let package = serde_json::json!({
            "schemas": [{"name": "a", "file": "schema/a.tql"}],
            "assembly": {"loadOrder": ["schema/a.tql", "data/x.tql", "manifests/m.json"]}
        });
        assert_eq!(
            assets(&package),
            (
                vec!["schema/a.tql".to_string()],
                vec!["data/x.tql".to_string()]
            )
        );
    }
}
