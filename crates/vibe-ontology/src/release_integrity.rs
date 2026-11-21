//! Version integrity: the checks that keep a package's version honest.
//!
//! Each rule answers a fault that shipped in a real release:
//!
//! 1. **Content changed under a released version** ([`validate_released`]): when
//!    tag `v<version>` exists, the shipped content at `HEAD` must equal that tag.
//! 2. **Stamps that disagree with `package.json`** ([`validate_version_stamps`]):
//!    every version stamp inside shipped files (manifest file name and fields,
//!    apply-unit headers, provenance records) must name the package version.
//! 3. **Listed files that do not exist** ([`validate_listed_paths`]): every path
//!    `package.json` or the manifest names must exist; in released mode every
//!    hash the manifest records must also match the file.
//! 4. **A bump smaller than the change** ([`check_bump`]): compared with the
//!    previous tag, a type, role or function added to the schema needs at least
//!    a minor bump; one removed or renamed, or a key changed, needs a major bump.
//!
//! Rules 2 and 3 (existence) run in every package validation. Rules 1, 3
//! (hashes) and 4 against the previous tag run in released mode, for a tree
//! that claims to be publishable (`ont validate-package --released`, the
//! pre-push hook, and `ont release` after it has committed and tagged); `ont
//! release` also applies rule 4 before it changes anything.
//!
//! Provenance `upstreamCommit` is deliberately not checked against `HEAD`: the
//! provenance is generated before the release commit exists, so it names the
//! release commit's parent. A commit cannot contain its own hash.

use std::collections::BTreeSet;
use std::fmt;
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::executable_package::{hash_bytes, APPLY_UNITS_ROOT};
use crate::package_release::resolve_module_repo_url;
use crate::version::SemanticVersion;

fn invalid<S: Into<String>>(message: S) -> Error {
    Error::Version(message.into())
}

// ---------------------------------------------------------------------------
// Rule 2: version stamps
// ---------------------------------------------------------------------------

fn manifest_version_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"-v(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)\.package-manifest\.json$")
            .expect("manifest version regex is valid")
    })
}

fn header_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?m)^#\s*(manifest|upstream-tag):\s*(\S+)\s*$").expect("header regex is valid")
    })
}

fn module_version_key_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r#"has moduleVersionKey "([^"@]+)@([^"]+)""#).expect("key regex is valid")
    })
}

fn manifest_path_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r#"has manifestPath "([^"]+)""#).expect("manifest path regex is valid")
    })
}

/// The manifest the package declares as current (`provenance.manifest`, else
/// the last `manifests` entry).
pub fn active_manifest(package: &Value) -> Option<String> {
    package
        .pointer("/provenance/manifest")
        .and_then(Value::as_str)
        .or_else(|| {
            package
                .get("manifests")
                .and_then(Value::as_array)
                .and_then(|manifests| manifests.last())
                .and_then(Value::as_str)
        })
        .map(str::to_string)
}

/// Every version stamp inside shipped files must equal the package version:
/// manifest file names, the manifest's `package.version` and `upstream.tag`,
/// and in every listed write file and apply unit the `# manifest:` and
/// `# upstream-tag:` headers, the package's own `moduleVersionKey` and its
/// `manifestPath`.
pub fn validate_version_stamps(repo_path: &Path, package: &Value) -> Result<()> {
    let Some(version) = package.get("version").and_then(Value::as_str) else {
        return Ok(());
    };
    let active = active_manifest(package);

    let mut manifest_paths: Vec<String> = string_array(package.get("manifests"))
        .into_iter()
        .map(str::to_string)
        .collect();
    manifest_paths.extend(active.clone());
    for path in &manifest_paths {
        if let Some(captures) = manifest_version_regex().captures(path) {
            if &captures[1] != version {
                return Err(invalid(format!(
                    "manifest {path} is named for version {} but package.json declares {version}",
                    &captures[1]
                )));
            }
        }
    }

    if let Some(active) = &active {
        if let Ok(text) = std::fs::read_to_string(repo_path.join(active)) {
            if let Ok(manifest) = serde_json::from_str::<Value>(&text) {
                if let Some(stamped) = manifest.pointer("/package/version").and_then(Value::as_str)
                {
                    if stamped != version {
                        return Err(invalid(format!(
                            "{active} package.version is {stamped} but package.json declares {version}"
                        )));
                    }
                }
                if let Some(tag) = manifest.pointer("/upstream/tag").and_then(Value::as_str) {
                    if tag != format!("v{version}") {
                        return Err(invalid(format!(
                            "{active} upstream.tag is {tag} but package.json declares {version}"
                        )));
                    }
                }
            }
        }
    }

    let repo_url = resolve_module_repo_url(package);
    for path in stamped_files(repo_path, package) {
        let Ok(text) = std::fs::read_to_string(repo_path.join(&path)) else {
            continue;
        };
        for captures in header_regex().captures_iter(&text) {
            let value = &captures[2];
            let wrong = match &captures[1] {
                "manifest" => active.as_deref().is_some_and(|active| value != active),
                _ => value != format!("v{version}"),
            };
            if wrong {
                return Err(invalid(format!(
                    "{path} is stamped '{}: {value}' but package.json declares {version}; \
                     run `ont prepare-package` (or `npm run refresh:package-contract`) to refresh it",
                    &captures[1]
                )));
            }
        }
        for captures in module_version_key_regex().captures_iter(&text) {
            if repo_url.as_deref() == Some(&captures[1]) && &captures[2] != version {
                return Err(invalid(format!(
                    "{path} records module version {} but package.json declares {version}",
                    &captures[2]
                )));
            }
        }
        if let Some(active) = &active {
            for captures in manifest_path_regex().captures_iter(&text) {
                let named = &captures[1];
                if manifest_version_regex().is_match(named) && named != active {
                    return Err(invalid(format!(
                        "{path} records manifest {named} but the package's manifest is {active}"
                    )));
                }
            }
        }
    }
    Ok(())
}

/// The write files that can carry stamps: listed data, the load order's
/// non-schema files, provenance files, and every file under the apply units.
fn stamped_files(repo_path: &Path, package: &Value) -> Vec<String> {
    let schemas: BTreeSet<&str> = package
        .get("schemas")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|schema| schema.get("file").and_then(Value::as_str))
        .collect();
    let mut paths: BTreeSet<String> = BTreeSet::new();
    for path in string_array(package.get("data"))
        .into_iter()
        .chain(string_array(package.pointer("/assembly/loadOrder")))
        .chain(string_array(package.pointer("/provenance/files")))
    {
        if path.ends_with(".tql") && !schemas.contains(path) {
            paths.insert(path.to_string());
        }
    }
    let units_root = repo_path.join(APPLY_UNITS_ROOT);
    let mut units = Vec::new();
    collect_files(repo_path, &units_root, &mut units);
    paths.extend(units);
    paths.into_iter().collect()
}

fn collect_files(repo_path: &Path, directory: &Path, found: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(repo_path, &path, found);
        } else if path.extension().is_some_and(|extension| extension == "tql") {
            if let Ok(relative) = path.strip_prefix(repo_path) {
                found.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: listed paths
// ---------------------------------------------------------------------------

/// Every path named in `package.json` (`queries`, `assembly.generatedArtifacts`,
/// migration units) and in the active manifest (`artifacts[].path`,
/// `upstream.sourceArtifacts[].path`) must exist. (Schemas, data, manifests,
/// provenance and the load order are checked by the package validator.)
pub fn validate_listed_paths(repo_path: &Path, package: &Value) -> Result<()> {
    for (field, paths) in [
        ("queries", string_array(package.get("queries"))),
        (
            "assembly.generatedArtifacts",
            string_array(package.pointer("/assembly/generatedArtifacts")),
        ),
    ] {
        for path in paths {
            if !repo_path.join(path).exists() {
                return Err(invalid(format!(
                    "package.json '{field}' lists {path}, which does not exist"
                )));
            }
        }
    }
    for plan in package
        .pointer("/migration/plans")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
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
                if let Some(path) = unit.get("path").and_then(Value::as_str) {
                    if !repo_path.join(path).exists() {
                        return Err(invalid(format!(
                            "package.json migration plan '{}' lists {path}, which does not exist",
                            plan.get("id").and_then(Value::as_str).unwrap_or("")
                        )));
                    }
                }
            }
        }
    }
    for (active, path, _) in manifest_entries(repo_path, package)? {
        if !repo_path.join(&path).exists() {
            return Err(invalid(format!(
                "{active} lists {path}, which does not exist"
            )));
        }
    }
    Ok(())
}

/// `(manifest, path, sha256)` for every artifact and source artifact the
/// active manifest records.
fn manifest_entries(
    repo_path: &Path,
    package: &Value,
) -> Result<Vec<(String, String, Option<String>)>> {
    let Some(active) = active_manifest(package) else {
        return Ok(Vec::new());
    };
    let Ok(text) = std::fs::read_to_string(repo_path.join(&active)) else {
        return Ok(Vec::new());
    };
    let manifest: Value = serde_json::from_str(&text)
        .map_err(|error| invalid(format!("invalid json in {active}: {error}")))?;
    let mut entries = Vec::new();
    for pointer in ["/artifacts", "/upstream/sourceArtifacts"] {
        for artifact in manifest
            .pointer(pointer)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(path) = artifact.get("path").and_then(Value::as_str) {
                entries.push((
                    active.clone(),
                    path.to_string(),
                    artifact
                        .get("sha256")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                ));
            }
        }
    }
    Ok(entries)
}

/// Every hash the manifest records must match the file: a hand-edit after the
/// release changes the file but not its recorded hash.
pub fn validate_manifest_hashes(repo_path: &Path, package: &Value) -> Result<()> {
    for (active, path, hash) in manifest_entries(repo_path, package)? {
        let Some(hash) = hash else { continue };
        let Ok(bytes) = std::fs::read(repo_path.join(&path)) else {
            return Err(invalid(format!(
                "{active} lists {path}, which does not exist"
            )));
        };
        if hash_bytes(&bytes) != hash {
            return Err(invalid(format!(
                "{path} differs from the hash {active} records; it was changed after the \
                 package was built (re-run `npm run refresh:package-contract`, or release a new version)"
            )));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Rule 4: bump size
// ---------------------------------------------------------------------------

/// The size of a release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Bump {
    Patch,
    Minor,
    Major,
}

impl fmt::Display for Bump {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Bump::Patch => "patch",
            Bump::Minor => "minor",
            Bump::Major => "major",
        })
    }
}

/// The bump between two versions (`None` when `to` is not greater).
pub fn bump_between(from: &SemanticVersion, to: &SemanticVersion) -> Option<Bump> {
    if to <= from {
        None
    } else if to.major != from.major {
        Some(Bump::Major)
    } else if to.minor != from.minor {
        Some(Bump::Minor)
    } else {
        Some(Bump::Patch)
    }
}

/// The labels a schema declares in its `define` blocks: types
/// (`entity Foo`, `relation bar`, `attribute baz`, `struct S`), roles
/// (`role bar:r`), functions (`function f`), and keys (`key Foo owns baz`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SchemaLabels {
    pub labels: BTreeSet<String>,
    pub keys: BTreeSet<String>,
}

const TYPE_KEYWORDS: [&str; 4] = ["entity", "relation", "attribute", "struct"];

/// Collects the labels declared in `define` blocks of `text`. Conservative and
/// textual: comments and string literals are ignored; `undefine`/`redefine`
/// blocks are skipped.
pub fn declared_labels(text: &str, into: &mut SchemaLabels) {
    let mut cleaned = String::new();
    for line in text.lines() {
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
                continue;
            }
            match character {
                '"' => in_string = true,
                '#' => break,
                _ => cleaned.push(character),
            }
        }
        cleaned.push('\n');
    }

    let mut in_define = false;
    let mut statement = String::new();
    for word in cleaned.split_inclusive(|character: char| character.is_whitespace()) {
        let bare = word.trim();
        if matches!(bare, "define" | "undefine" | "redefine") && statement.trim().is_empty() {
            in_define = bare == "define";
            statement.clear();
            continue;
        }
        statement.push_str(word);
        if bare.ends_with(';') {
            if in_define {
                record_statement(statement.trim().trim_end_matches(';'), into);
            }
            statement.clear();
        }
    }
}

fn record_statement(statement: &str, into: &mut SchemaLabels) {
    let mut tokens = statement.split_whitespace();
    let Some(first) = tokens.next() else { return };
    if first == "fun" {
        if let Some(rest) = statement.strip_prefix("fun") {
            let name: String = rest
                .trim_start()
                .chars()
                .take_while(|character| {
                    character.is_alphanumeric() || matches!(character, '_' | '-')
                })
                .collect();
            if !name.is_empty() {
                into.labels.insert(format!("function {name}"));
            }
        }
        return;
    }
    let (label, body) = if TYPE_KEYWORDS.contains(&first) {
        let Some(label) = tokens.next() else { return };
        let label = label.trim_end_matches(',');
        into.labels.insert(format!("{first} {label}"));
        let body = statement
            .split_once(label)
            .map(|(_, body)| body)
            .unwrap_or("");
        (label.to_string(), body)
    } else {
        // `Label owns x, plays y;` extends an existing type.
        let is_label = first
            .chars()
            .all(|character| character.is_alphanumeric() || matches!(character, '_' | '-'));
        let next = tokens.next().unwrap_or("");
        if !is_label || !matches!(next, "sub" | "owns" | "plays" | "relates" | "value") {
            return;
        }
        (
            first.to_string(),
            statement
                .split_once(first)
                .map(|(_, body)| body)
                .unwrap_or(""),
        )
    };
    for clause in body.split(',') {
        let mut words = clause.split_whitespace();
        match words.next() {
            Some("relates") => {
                if let Some(role) = words.next() {
                    into.labels.insert(format!("role {label}:{role}"));
                }
            }
            Some("owns") => {
                if let Some(attribute) = words.next() {
                    if clause.contains("@key") {
                        into.keys.insert(format!("key {label} owns {attribute}"));
                    }
                }
            }
            _ => {}
        }
    }
}

/// The smallest bump that may carry the change from `old` to `new`, with the
/// labels that require it.
pub fn required_bump(old: &SchemaLabels, new: &SchemaLabels) -> (Bump, Vec<String>) {
    let removed: Vec<String> = old.labels.difference(&new.labels).cloned().collect();
    // A key on a type the previous release did not have is part of adding that
    // type, not a key change.
    let existed = |key: &String| {
        let owner = key
            .strip_prefix("key ")
            .and_then(|rest| rest.split_whitespace().next())
            .unwrap_or("");
        TYPE_KEYWORDS
            .iter()
            .any(|kind| old.labels.contains(&format!("{kind} {owner}")))
    };
    let keys_changed: Vec<String> = old
        .keys
        .symmetric_difference(&new.keys)
        .filter(|key| existed(key))
        .cloned()
        .collect();
    if !removed.is_empty() || !keys_changed.is_empty() {
        let mut reasons: Vec<String> = removed
            .into_iter()
            .map(|label| format!("removes {label}"))
            .collect();
        reasons.extend(keys_changed.into_iter().map(|key| format!("changes {key}")));
        return (Bump::Major, reasons);
    }
    let added: Vec<String> = new.labels.difference(&old.labels).cloned().collect();
    if !added.is_empty() {
        return (
            Bump::Minor,
            added
                .into_iter()
                .map(|label| format!("adds {label}"))
                .collect(),
        );
    }
    (Bump::Patch, Vec::new())
}

/// The labels of the package's schema files at `git_ref` (or the working tree
/// when `None`), per that state's own `package.json`.
pub fn schema_labels_at(repo_path: &Path, git_ref: Option<&str>) -> Result<SchemaLabels> {
    let read = |path: &str| -> Result<String> {
        match git_ref {
            Some(git_ref) => git_show(repo_path, git_ref, path),
            None => std::fs::read_to_string(repo_path.join(path))
                .map_err(|error| invalid(format!("failed to read {path}: {error}"))),
        }
    };
    let package: Value = serde_json::from_str(&read("package.json")?)
        .map_err(|error| invalid(format!("invalid package.json: {error}")))?;
    let mut labels = SchemaLabels::default();
    for schema in package
        .get("schemas")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(file) = schema.get("file").and_then(Value::as_str) {
            if let Ok(text) = read(file) {
                declared_labels(&text, &mut labels);
            }
        }
    }
    Ok(labels)
}

/// Refuses a release from `from_version` (tagged `from_tag`) to `to_version`
/// that is smaller than the schema change between the tag and `new` labels.
pub fn check_bump(
    from_tag: &str,
    old: &SchemaLabels,
    new: &SchemaLabels,
    from_version: &SemanticVersion,
    to_version: &SemanticVersion,
) -> Result<()> {
    let Some(bump) = bump_between(from_version, to_version) else {
        return Ok(());
    };
    let (required, reasons) = required_bump(old, new);
    if bump < required {
        return Err(invalid(format!(
            "a {bump} release ({from_version} -> {to_version}) is too small for the schema change \
             since {from_tag}, which needs at least a {required} bump: {}",
            reasons.join(", ")
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Rule 1 and released mode
// ---------------------------------------------------------------------------

/// The shipped content: `package.json`, every path it lists, and the
/// directories whose files ship or prove the package.
fn shipped_pathspecs(package: &Value) -> Vec<String> {
    let mut paths: BTreeSet<String> = [
        "package.json",
        "schema",
        "data",
        "queries",
        "competency",
        "fixtures",
        "manifests",
        "migrations",
        APPLY_UNITS_ROOT,
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    for schema in package
        .get("schemas")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(file) = schema.get("file").and_then(Value::as_str) {
            paths.insert(file.to_string());
        }
    }
    for pointer in ["/data", "/queries", "/manifests", "/assembly/loadOrder"] {
        paths.extend(
            string_array(package.pointer(pointer))
                .into_iter()
                .map(str::to_string),
        );
    }
    paths.into_iter().collect()
}

/// Rule 1: when tag `v<version>` exists, the shipped content at `HEAD` must
/// equal it.
pub fn validate_unchanged_since_tag(repo_path: &Path, package: &Value) -> Result<()> {
    let Some(version) = package.get("version").and_then(Value::as_str) else {
        return Ok(());
    };
    let tag = format!("v{version}");
    if !tag_exists(repo_path, &tag)? {
        return Ok(());
    }
    let mut args = vec![
        "diff".to_string(),
        "--name-only".to_string(),
        tag.clone(),
        "HEAD".to_string(),
        "--".to_string(),
    ];
    args.extend(shipped_pathspecs(package));
    let changed = git(repo_path, &args)?;
    let changed: Vec<&str> = changed
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    if !changed.is_empty() {
        let shown: Vec<&str> = changed.iter().take(5).copied().collect();
        let more = if changed.len() > shown.len() {
            format!(" and {} more", changed.len() - shown.len())
        } else {
            String::new()
        };
        return Err(invalid(format!(
            "package content differs from tag {tag} but package.json still declares {version}; \
             every change needs a version bump (run `ont release --bump …`). Changed: {}{more}",
            shown.join(", ")
        )));
    }
    Ok(())
}

/// Released mode: everything a publishable tree must satisfy beyond ordinary
/// validation. Rule 1 (no change under a released version); manifest hashes
/// match the files; `competency/last-run.json` names this version; and the
/// bump from the previous tag is at least as large as the schema change.
pub fn validate_released(repo_path: &Path, package: &Value) -> Result<()> {
    validate_unchanged_since_tag(repo_path, package)?;
    validate_manifest_hashes(repo_path, package)?;

    let version_text = package.get("version").and_then(Value::as_str).unwrap_or("");
    let last_run_path = repo_path.join(crate::competency_questions::LAST_RUN_PATH);
    if let Ok(text) = std::fs::read_to_string(&last_run_path) {
        if let Ok(last_run) = serde_json::from_str::<Value>(&text) {
            if let Some(stamped) = last_run.get("version").and_then(Value::as_str) {
                if stamped != version_text {
                    return Err(invalid(format!(
                        "{} records version {stamped} but package.json declares {version_text}",
                        crate::competency_questions::LAST_RUN_PATH
                    )));
                }
            }
        }
    }

    let version = SemanticVersion::parse(version_text.trim_start_matches('v'))?;
    if let Some((previous_tag, previous)) = previous_tag(repo_path, &version)? {
        let old = schema_labels_at(repo_path, Some(&previous_tag))?;
        let new = schema_labels_at(repo_path, Some("HEAD"))?;
        check_bump(&previous_tag, &old, &new, &previous, &version)?;
    }
    Ok(())
}

/// The highest `v*.*.*` tag below `version`.
pub fn previous_tag(
    repo_path: &Path,
    version: &SemanticVersion,
) -> Result<Option<(String, SemanticVersion)>> {
    let output = git(
        repo_path,
        &[
            "tag".to_string(),
            "--list".to_string(),
            "v*.*.*".to_string(),
        ],
    )?;
    let mut best: Option<(String, SemanticVersion)> = None;
    for tag in output.lines().map(str::trim) {
        let Ok(parsed) = SemanticVersion::parse(tag.trim_start_matches('v')) else {
            continue;
        };
        if parsed < *version && best.as_ref().is_none_or(|(_, best)| parsed > *best) {
            best = Some((tag.to_string(), parsed));
        }
    }
    Ok(best)
}

pub fn tag_exists(repo_path: &Path, tag: &str) -> Result<bool> {
    let output = Command::new("git")
        .args(["rev-parse", "-q", "--verify", &format!("refs/tags/{tag}")])
        .current_dir(repo_path)
        .output()
        .map_err(|error| invalid(format!("git rev-parse failed: {error}")))?;
    Ok(output.status.success())
}

fn git_show(repo_path: &Path, git_ref: &str, path: &str) -> Result<String> {
    git(
        repo_path,
        &["show".to_string(), format!("{git_ref}:{path}")],
    )
}

fn git(repo_path: &Path, args: &[String]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_path)
        .output()
        .map_err(|error| invalid(format!("git {} failed: {error}", args.join(" "))))?;
    if !output.status.success() {
        return Err(invalid(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
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

    fn write(root: &Path, relative_path: &str, contents: &str) {
        let path = root.join(relative_path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    fn run_git(root: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(status.status.success(), "git {args:?} failed");
    }

    fn message(result: Result<()>) -> String {
        match result {
            Err(Error::Version(message)) => message,
            other => panic!("expected a version error, got {other:?}"),
        }
    }

    const SCHEMA: &str =
        "define\n  attribute name, value string;\n  entity Book, owns name @key;\n  \
                          relation authorship, relates work, relates author;\n  \
                          fun books() -> { Book }: match $b isa Book; return { $b };\n";

    fn package(version: &str) -> Value {
        json!({
            "name": "custom",
            "version": version,
            "source": "https://example.com/ontology-custom",
            "schemas": [{"name": "custom", "file": "schema/custom.tql"}],
            "data": ["data/provenance.tql"],
            "manifests": [format!("manifests/custom-v{version}.package-manifest.json")],
            "provenance": {"manifest": format!("manifests/custom-v{version}.package-manifest.json")}
        })
    }

    fn manifest(version: &str, artifacts: Value) -> String {
        json!({
            "package": {"name": "custom", "version": version},
            "upstream": {"tag": format!("v{version}"), "sourceArtifacts": []},
            "artifacts": artifacts
        })
        .to_string()
    }

    fn provenance(version: &str) -> String {
        format!(
            "# upstream-tag: v{version}\n# manifest: manifests/custom-v{version}.package-manifest.json\n\
             put $v isa OntologyModuleVersion, has moduleVersionKey \"https://example.com/ontology-custom@{version}\",\n  \
             has manifestPath \"manifests/custom-v{version}.package-manifest.json\";\n"
        )
    }

    /// A package at `version` whose files all carry that version.
    fn fixture(version: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "schema/custom.tql", SCHEMA);
        write(root, "data/provenance.tql", &provenance(version));
        write(
            root,
            &format!("manifests/custom-v{version}.package-manifest.json"),
            &manifest(version, json!([])),
        );
        write(
            root,
            "package.json",
            &serde_json::to_string_pretty(&package(version)).unwrap(),
        );
        dir
    }

    #[test]
    fn stamps_that_match_the_version_pass() {
        let dir = fixture("1.0.2");
        validate_version_stamps(dir.path(), &package("1.0.2")).unwrap();
    }

    #[test]
    fn rejects_a_manifest_named_for_another_version() {
        let dir = fixture("1.0.2");
        let mut package = package("1.0.2");
        package["manifests"] = json!(["manifests/custom-v1.0.1.package-manifest.json"]);
        package["provenance"]["manifest"] = json!("manifests/custom-v1.0.1.package-manifest.json");
        assert_eq!(
            message(validate_version_stamps(dir.path(), &package)),
            "manifest manifests/custom-v1.0.1.package-manifest.json is named for version 1.0.1 \
             but package.json declares 1.0.2"
        );
    }

    #[test]
    fn rejects_manifest_fields_for_another_version() {
        let dir = fixture("1.0.2");
        write(
            dir.path(),
            "manifests/custom-v1.0.2.package-manifest.json",
            &manifest("1.0.1", json!([])),
        );
        assert_eq!(
            message(validate_version_stamps(dir.path(), &package("1.0.2"))),
            "manifests/custom-v1.0.2.package-manifest.json package.version is 1.0.1 but \
             package.json declares 1.0.2"
        );
    }

    #[test]
    fn rejects_a_stale_apply_unit_header() {
        let dir = fixture("1.0.2");
        write(
            dir.path(),
            "generated/apply-units/data/docs/0001.tql",
            "# Generated executable apply unit from data/docs.tql\n\
             # manifest: manifests/custom-v1.0.1.package-manifest.json\n\nput $x isa Book;\n",
        );
        assert_eq!(
            message(validate_version_stamps(dir.path(), &package("1.0.2"))),
            "generated/apply-units/data/docs/0001.tql is stamped 'manifest: \
             manifests/custom-v1.0.1.package-manifest.json' but package.json declares 1.0.2; run \
             `ont prepare-package` (or `npm run refresh:package-contract`) to refresh it"
        );
    }

    #[test]
    fn rejects_stale_provenance_records() {
        let dir = fixture("1.0.2");
        write(
            dir.path(),
            "data/provenance.tql",
            &provenance("1.0.2").replace("ontology-custom@1.0.2", "ontology-custom@1.0.1"),
        );
        assert_eq!(
            message(validate_version_stamps(dir.path(), &package("1.0.2"))),
            "data/provenance.tql records module version 1.0.1 but package.json declares 1.0.2"
        );
    }

    #[test]
    fn rejects_a_listed_file_that_does_not_exist() {
        let dir = fixture("1.0.2");
        write(
            dir.path(),
            "manifests/custom-v1.0.2.package-manifest.json",
            &manifest(
                "1.0.2",
                json!([{"kind": "migration", "path": "migrations/v1.0.1-to-v1.0.2.tql"}]),
            ),
        );
        assert_eq!(
            message(validate_listed_paths(dir.path(), &package("1.0.2"))),
            "manifests/custom-v1.0.2.package-manifest.json lists migrations/v1.0.1-to-v1.0.2.tql, \
             which does not exist"
        );
        let mut package = package("1.0.2");
        package["queries"] = json!(["queries/gone.tql"]);
        write(
            dir.path(),
            "manifests/custom-v1.0.2.package-manifest.json",
            &manifest("1.0.2", json!([])),
        );
        assert_eq!(
            message(validate_listed_paths(dir.path(), &package)),
            "package.json 'queries' lists queries/gone.tql, which does not exist"
        );
    }

    #[test]
    fn rejects_a_file_edited_after_its_hash_was_recorded() {
        let dir = fixture("1.0.2");
        let hash = hash_bytes(SCHEMA.as_bytes());
        write(
            dir.path(),
            "manifests/custom-v1.0.2.package-manifest.json",
            &manifest(
                "1.0.2",
                json!([{"path": "schema/custom.tql", "sha256": hash}]),
            ),
        );
        validate_manifest_hashes(dir.path(), &package("1.0.2")).unwrap();
        write(
            dir.path(),
            "schema/custom.tql",
            &format!("{SCHEMA}# edited\n"),
        );
        assert_eq!(
            message(validate_manifest_hashes(dir.path(), &package("1.0.2"))),
            "schema/custom.tql differs from the hash manifests/custom-v1.0.2.package-manifest.json \
             records; it was changed after the package was built (re-run `npm run \
             refresh:package-contract`, or release a new version)"
        );
    }

    #[test]
    fn rejects_content_changed_under_a_released_version() {
        let dir = fixture("1.0.1");
        let root = dir.path();
        run_git(root, &["init", "-b", "main"]);
        run_git(root, &["config", "user.name", "Fixture"]);
        run_git(root, &["config", "user.email", "fixture@example.com"]);
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-m", "Release custom v1.0.1"]);
        run_git(root, &["tag", "v1.0.1"]);
        validate_unchanged_since_tag(root, &package("1.0.1")).unwrap();

        // A docs-only change is not shipped content.
        write(root, "docs/notes.md", "notes\n");
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-m", "docs"]);
        validate_unchanged_since_tag(root, &package("1.0.1")).unwrap();

        write(
            root,
            "schema/custom.tql",
            &format!("{SCHEMA}  entity Shelf;\n"),
        );
        run_git(root, &["commit", "-am", "add Shelf"]);
        assert_eq!(
            message(validate_unchanged_since_tag(root, &package("1.0.1"))),
            "package content differs from tag v1.0.1 but package.json still declares 1.0.1; every \
             change needs a version bump (run `ont release --bump …`). Changed: schema/custom.tql"
        );

        // Released mode on a bumped version compares the bump with the schema
        // change since v1.0.1: a patch is too small for an added entity.
        write(
            root,
            "package.json",
            &serde_json::to_string_pretty(&package("1.0.2")).unwrap(),
        );
        assert_eq!(
            message(validate_released(root, &package("1.0.2"))),
            "a patch release (1.0.1 -> 1.0.2) is too small for the schema change since v1.0.1, \
             which needs at least a minor bump: adds entity Shelf"
        );
    }

    fn labels(text: &str) -> SchemaLabels {
        let mut labels = SchemaLabels::default();
        declared_labels(text, &mut labels);
        labels
    }

    #[test]
    fn collects_labels_from_define_blocks() {
        let found = labels(&format!(
            "# entity Ignored;\n{SCHEMA}\nundefine\n  entity Gone;\n\ndefine\n  Book owns isbn @key;\n"
        ));
        assert_eq!(
            found.labels.into_iter().collect::<Vec<_>>(),
            vec![
                "attribute name",
                "entity Book",
                "function books",
                "relation authorship",
                "role authorship:author",
                "role authorship:work",
            ]
        );
        assert_eq!(
            found.keys.into_iter().collect::<Vec<_>>(),
            vec!["key Book owns isbn", "key Book owns name"]
        );
    }

    #[test]
    fn bump_must_cover_the_schema_change() {
        let old = labels(SCHEMA);
        let v = |text: &str| SemanticVersion::parse(text).unwrap();

        let added = labels(&format!(
            "{SCHEMA}  entity Shelf;\n  relation shelving, relates shelf;\n"
        ));
        assert_eq!(
            message(check_bump("v1.0.1", &old, &added, &v("1.0.1"), &v("1.0.2"))),
            "a patch release (1.0.1 -> 1.0.2) is too small for the schema change since v1.0.1, \
             which needs at least a minor bump: adds entity Shelf, adds relation shelving, adds \
             role shelving:shelf"
        );
        check_bump("v1.0.1", &old, &added, &v("1.0.1"), &v("1.1.0")).unwrap();

        let renamed = labels(&SCHEMA.replace("entity Book", "entity Volume"));
        assert_eq!(
            message(check_bump(
                "v1.0.1",
                &old,
                &renamed,
                &v("1.0.1"),
                &v("1.1.0")
            )),
            "a minor release (1.0.1 -> 1.1.0) is too small for the schema change since v1.0.1, \
             which needs at least a major bump: removes entity Book, changes key Book owns name"
        );

        let rekeyed = labels(&SCHEMA.replace("owns name @key", "owns name"));
        assert_eq!(
            message(check_bump(
                "v1.0.1",
                &old,
                &rekeyed,
                &v("1.0.1"),
                &v("1.1.0")
            )),
            "a minor release (1.0.1 -> 1.1.0) is too small for the schema change since v1.0.1, \
             which needs at least a major bump: changes key Book owns name"
        );
        check_bump("v1.0.1", &old, &rekeyed, &v("1.0.1"), &v("2.0.0")).unwrap();

        // A new type with its own key is an addition, not a key change.
        let keyed = labels(&format!("{SCHEMA}  entity Shelf, owns name @key;\n"));
        assert_eq!(
            required_bump(&old, &keyed),
            (Bump::Minor, vec!["adds entity Shelf".to_string()])
        );

        // Data, docs and query changes leave the labels alone: a patch is fine.
        check_bump("v1.0.1", &old, &labels(SCHEMA), &v("1.0.1"), &v("1.0.2")).unwrap();
    }
}
