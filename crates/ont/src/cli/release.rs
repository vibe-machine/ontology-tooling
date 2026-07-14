use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use clap::Args;
use serde::Serialize;
use serde_json::Value;
use vibe_ontology::bootstrap_uniqueness::validate_bootstrap_uniqueness;
use vibe_ontology::executable_package::prepare_executable_package;
use vibe_ontology::migration_diff::generate_migration_diff;
use vibe_ontology::package_release::{
    expected_release_commit_message, migration_preflight_assertion, migration_verify_assertion,
    plan_package_release, release_scripts_to_run, resolve_module_repo_url,
    rewrite_compatible_migration_unit_paths, strip_migration_metadata, ReleasePlan, Rename,
};
use vibe_ontology::package_validator::validate_package_contract;
use vibe_ontology::release_args::{validate_release_args, ReleaseArgs as ValidatedReleaseArgs};

use super::{emit_json, Cli, Format};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Args, Debug, Clone)]
pub struct ReleaseArgs {
    /// Path to the ontology repository to release or validate.
    #[arg(long)]
    pub repo: String,

    /// Semantic-version bump kind (patch, minor, or major).
    #[arg(long)]
    pub bump: Option<String>,

    /// Explicit semantic version to release.
    #[arg(long)]
    pub version: Option<String>,

    /// Print the release plan without mutating the repository.
    #[arg(long)]
    pub dry_run: bool,

    /// Do not push the release commit and tag to origin (push is the default).
    #[arg(long = "no-push")]
    pub no_push: bool,

    /// Validate the current HEAD in a detached temporary worktree.
    #[arg(long)]
    pub validate_only: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseSummary {
    repo_path: PathBuf,
    package_name: String,
    current_version: String,
    next_version: Option<String>,
    tag_name: Option<String>,
    push: bool,
    resume_existing_version: bool,
    renamed_paths: Vec<Rename>,
    scripts: Vec<String>,
    mode: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    migration_diff: Option<String>,
}

pub fn run(cli: &Cli, args: &ReleaseArgs) -> Result<()> {
    validate_release_args(&ValidatedReleaseArgs {
        repo: Some(args.repo.clone()),
        bump: args.bump.clone(),
        version: args.version.clone(),
        dry_run: args.dry_run,
        validate_only: args.validate_only,
        push: !args.no_push,
        help: false,
    })?;

    let summary = execute_release(args)?;
    match Format::resolve(cli.format) {
        Format::Json => emit_json(&summary)?,
        Format::Text => emit_text(&summary),
    }
    Ok(())
}

fn execute_release(options: &ReleaseArgs) -> Result<ReleaseSummary> {
    let repo_path = normalize_lexically(
        &std::path::absolute(&options.repo)
            .with_context(|| format!("resolve repository path {}", options.repo))?,
    );
    let package_json_path = repo_path.join("package.json");
    let package_json = read_json(&package_json_path)?;
    let scripts_to_run = release_scripts_to_run(&package_json);
    let package_name = package_json
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("undefined")
        .to_string();
    let current_version = package_json
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or("undefined")
        .to_string();

    if options.validate_only {
        let summary = ReleaseSummary {
            repo_path: repo_path.clone(),
            package_name,
            current_version,
            next_version: None,
            tag_name: None,
            push: false,
            resume_existing_version: false,
            renamed_paths: Vec::new(),
            scripts: scripts_to_run,
            mode: "validate-only",
            migration_diff: None,
        };

        with_validation_worktree(&repo_path, |worktree_path| {
            run_release_validation(worktree_path, None::<fn(&Path) -> Result<()>>)
        })?;
        return Ok(summary);
    }

    let plan = plan_package_release(
        &package_json,
        options.bump.as_deref(),
        options.version.as_deref(),
    )?;
    let mut summary = build_summary(
        &repo_path,
        package_name.clone(),
        &plan,
        !options.no_push,
        scripts_to_run,
    );
    summary.mode = if options.dry_run {
        "dry-run"
    } else {
        "release"
    };
    summary.resume_existing_version = plan.resume_existing_version;

    if options.dry_run {
        return Ok(summary);
    }

    assert_clean_working_tree(&repo_path)?;
    let tag_name = summary
        .tag_name
        .as_deref()
        .expect("release summaries always have a tag");
    assert_tag_does_not_exist(&repo_path, tag_name)?;

    if plan.resume_existing_version {
        assert_head_matches_release_commit(&repo_path, &package_name, &plan.next_version)?;
        with_validation_worktree(&repo_path, |worktree_path| {
            run_release_validation(worktree_path, None::<fn(&Path) -> Result<()>>)
        })?;
    } else {
        write_json(&package_json_path, &plan.next_package_json)?;
        apply_rename_plan(&repo_path, &plan.rename_plan)?;

        run_release_validation(
            &repo_path,
            Some(|repo: &Path| {
                let migration_path =
                    generate_migration_diff(repo, &plan.current_version, &plan.next_version)?;
                if let Some(migration_path) = migration_path {
                    write_migration_assertions(
                        repo,
                        &plan.next_package_json,
                        &plan.current_version,
                        &plan.next_version,
                    )?;
                    let package_json_path = repo.join("package.json");
                    let current_package_json = read_json(&package_json_path)?;
                    let rewritten_package_json = rewrite_compatible_migration_unit_paths(
                        &current_package_json,
                        &migration_path,
                        &plan.current_version,
                        &plan.next_version,
                    );
                    if rewritten_package_json != current_package_json {
                        write_json(&package_json_path, &rewritten_package_json)?;
                    }
                    summary.migration_diff = Some(migration_path);
                    return Ok(());
                }

                let package_json_path = repo.join("package.json");
                let current_package_json = read_json(&package_json_path)?;
                if current_package_json
                    .get("migration")
                    .is_some_and(json_truthy)
                {
                    write_json(
                        &package_json_path,
                        &strip_migration_metadata(&current_package_json),
                    )?;
                    remove_path(&repo.join("migrations"))?;
                    run_package_script(repo, "refresh:package-contract")?;
                }
                Ok(())
            }),
        )?;

        run_command("git", ["add", "-A"], &repo_path, false)?;
        let commit_message = expected_release_commit_message(&package_name, &plan.next_version);
        run_command(
            "git",
            ["commit", "-m", commit_message.as_str()],
            &repo_path,
            false,
        )?;
    }

    let tag_name = summary
        .tag_name
        .as_deref()
        .expect("release summaries always have a tag");
    run_command("git", ["tag", tag_name], &repo_path, false)?;

    if !options.no_push {
        push_release(&repo_path, tag_name)?;
    }

    Ok(summary)
}

/// Collapse `.` and `..` components lexically, matching Node `path.resolve`
/// (which normalizes without touching the filesystem). `std::path::absolute`
/// makes the path absolute but deliberately leaves `..` in place, which would
/// both pollute the reported `repoPath` and make `.parent()` lexically wrong
/// for the sibling-repo / worktree placement logic.
fn normalize_lexically(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                // Only pop a real directory name; never pop past a root/prefix.
                if matches!(
                    normalized.components().next_back(),
                    Some(Component::Normal(_))
                ) {
                    normalized.pop();
                } else if normalized.as_os_str().is_empty() {
                    normalized.push(Component::ParentDir);
                }
            }
            Component::CurDir => {}
            other => normalized.push(other),
        }
    }
    normalized
}

fn read_json(path: &Path) -> Result<Value> {
    let text =
        fs::read_to_string(path).with_context(|| format!("read JSON file {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parse JSON file {}", path.display()))
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    fs::write(path, text).with_context(|| format!("write JSON file {}", path.display()))
}

fn run_command<I, S>(command: &str, args: I, cwd: &Path, capture_output: bool) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args: Vec<_> = args.into_iter().collect();
    let mut child = Command::new(command);
    child.args(&args).current_dir(cwd);

    if capture_output {
        let output = child
            .stdin(Stdio::null())
            .output()
            .with_context(|| format!("execute {command} in {}", cwd.display()))?;
        if !output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let details = match (stdout.trim().is_empty(), stderr.trim().is_empty()) {
                (false, false) => format!("\n{}\n{}", stdout.trim_end(), stderr.trim_end()),
                (false, true) => format!("\n{}", stdout.trim_end()),
                (true, false) => format!("\n{}", stderr.trim_end()),
                (true, true) => String::new(),
            };
            bail!("{command} exited with status {}{details}", output.status);
        }
        return String::from_utf8(output.stdout)
            .with_context(|| format!("decode {command} stdout as UTF-8"));
    }

    let status = child
        .status()
        .with_context(|| format!("execute {command} in {}", cwd.display()))?;
    if !status.success() {
        bail!("{command} exited with status {status}");
    }
    Ok(String::new())
}

fn run_package_script(repo_path: &Path, script_name: &str) -> Result<()> {
    run_command("npm", ["run", script_name], repo_path, false)?;
    Ok(())
}

fn run_release_validation<F>(repo_path: &Path, after_refresh: Option<F>) -> Result<()>
where
    F: FnOnce(&Path) -> Result<()>,
{
    run_package_script(repo_path, "refresh:package-contract")?;

    if let Some(after_refresh) = after_refresh {
        after_refresh(repo_path)?;
    }
    prepare_executable_package(repo_path, None)?;

    let package_json = validate_package_contract(repo_path)?;
    validate_bootstrap_uniqueness(repo_path)?;

    for script_name in release_scripts_to_run(&package_json) {
        if script_name == "refresh:package-contract" {
            continue;
        }
        run_package_script(repo_path, &script_name)?;
    }
    Ok(())
}

fn with_validation_worktree<T, F>(repo_path: &Path, callback: F) -> Result<T>
where
    F: FnOnce(&Path) -> Result<T>,
{
    let workspace_root = repo_path
        .parent()
        .context("Target repository path has no parent directory")?;
    let temp_root = create_temp_dir(workspace_root)?;
    let repo_name = repo_path
        .file_name()
        .context("Target repository path has no file name")?;
    let worktree_path = temp_root.join(repo_name);

    let result = (|| {
        run_command(
            "git",
            [
                OsStr::new("worktree"),
                OsStr::new("add"),
                OsStr::new("--detach"),
                worktree_path.as_os_str(),
                OsStr::new("HEAD"),
            ],
            repo_path,
            false,
        )?;
        link_sibling_ontology_repos(repo_path, &worktree_path)?;
        if should_install_node_dependencies(&worktree_path)? {
            run_command(
                "npm",
                ["install", "--no-audit", "--no-fund"],
                &worktree_path,
                false,
            )?;
        }
        callback(&worktree_path)
    })();

    if run_command(
        "git",
        [
            OsStr::new("worktree"),
            OsStr::new("remove"),
            OsStr::new("--force"),
            worktree_path.as_os_str(),
        ],
        repo_path,
        false,
    )
    .is_err()
    {
        let _ = remove_path(&worktree_path);
    }
    let cleanup_result = remove_path(&temp_root);

    match (result, cleanup_result) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Ok(value), Ok(())) => Ok(value),
    }
}

fn create_temp_dir(workspace_root: &Path) -> Result<PathBuf> {
    for _ in 0..100 {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = workspace_root.join(format!(
            ".ontology-release-check-{}-{timestamp}-{sequence}",
            std::process::id()
        ));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("create temporary directory {}", path.display()));
            }
        }
    }
    bail!(
        "failed to create temporary release worktree under {}",
        workspace_root.display()
    )
}

fn link_sibling_ontology_repos(repo_path: &Path, worktree_path: &Path) -> Result<()> {
    let workspace_root = repo_path
        .parent()
        .context("Target repository path has no parent directory")?;
    let worktree_root = worktree_path
        .parent()
        .context("Worktree path has no parent directory")?;
    let repo_name = repo_path.file_name();

    for entry in fs::read_dir(workspace_root)
        .with_context(|| format!("read workspace directory {}", workspace_root.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with("ontology-") || Some(name.as_os_str()) == repo_name {
            continue;
        }

        let source_path = workspace_root.join(&name);
        let target_path = worktree_root.join(&name);
        if path_exists(&target_path) {
            continue;
        }
        symlink_dir(&source_path, &target_path)?;
    }
    Ok(())
}

#[cfg(unix)]
fn symlink_dir(source: &Path, target: &Path) -> Result<()> {
    std::os::unix::fs::symlink(source, target)
        .with_context(|| format!("symlink {} to {}", target.display(), source.display()))
}

#[cfg(windows)]
fn symlink_dir(source: &Path, target: &Path) -> Result<()> {
    std::os::windows::fs::symlink_dir(source, target)
        .with_context(|| format!("symlink {} to {}", target.display(), source.display()))
}

fn should_install_node_dependencies(repo_path: &Path) -> Result<bool> {
    let package_json = read_json(&repo_path.join("package.json"))?;
    Ok([
        "dependencies",
        "devDependencies",
        "optionalDependencies",
        "peerDependencies",
    ]
    .iter()
    .any(|field| {
        package_json
            .get(field)
            .and_then(Value::as_object)
            .is_some_and(|dependencies| !dependencies.is_empty())
    }))
}

fn apply_rename_plan(repo_path: &Path, rename_plan: &[Rename]) -> Result<()> {
    for rename in rename_plan {
        let from_path = repo_path.join(&rename.from);
        let to_path = repo_path.join(&rename.to);
        if !path_exists(&from_path) || path_exists(&to_path) {
            continue;
        }
        if let Some(parent) = to_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create directory {}", parent.display()))?;
        }
        fs::rename(&from_path, &to_path)
            .with_context(|| format!("rename {} to {}", from_path.display(), to_path.display()))?;
    }
    Ok(())
}

fn path_exists(path: &Path) -> bool {
    fs::metadata(path).is_ok()
}

fn remove_path(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("inspect path {}", path.display()));
        }
    };
    if metadata.file_type().is_symlink() || metadata.is_file() {
        fs::remove_file(path).with_context(|| format!("remove path {}", path.display()))
    } else {
        fs::remove_dir_all(path).with_context(|| format!("remove path {}", path.display()))
    }
}

fn assert_clean_working_tree(repo_path: &Path) -> Result<()> {
    let output = run_command("git", ["status", "--porcelain"], repo_path, true)?;
    let output = output.trim();
    if !output.is_empty() {
        bail!("Target repo has uncommitted changes:\n{output}");
    }
    Ok(())
}

fn assert_tag_does_not_exist(repo_path: &Path, tag_name: &str) -> Result<()> {
    let tags = run_command("git", ["tag", "--list", tag_name], repo_path, true)?;
    if tags.trim() == tag_name {
        bail!("Git tag already exists: {tag_name}");
    }
    Ok(())
}

fn assert_head_matches_release_commit(
    repo_path: &Path,
    package_name: &str,
    version: &str,
) -> Result<()> {
    let subject = run_command("git", ["log", "-1", "--pretty=%s"], repo_path, true)?;
    let subject = subject.trim();
    let expected = expected_release_commit_message(package_name, version);
    if subject != expected {
        bail!(
            "Cannot resume release for v{version}: HEAD commit is \"{subject}\", expected \"{expected}\""
        );
    }
    Ok(())
}

fn write_migration_assertions(
    repo_path: &Path,
    package_json: &Value,
    current_version: &str,
    next_version: &str,
) -> Result<()> {
    let module_repo_url = resolve_module_repo_url(package_json)
        .filter(|url| !url.trim().is_empty())
        .context(
            "Target package.json is missing source/upstream repo URL required for migration assertions",
        )?;
    let preflight_path = repo_path.join(format!(
        "migrations/preflight/assert-v{current_version}-module-version.tql"
    ));
    let verify_path = repo_path.join(format!(
        "migrations/verify/assert-v{next_version}-module-version.tql"
    ));
    if let Some(parent) = preflight_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if let Some(parent) = verify_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        &preflight_path,
        migration_preflight_assertion(&module_repo_url, current_version, next_version),
    )?;
    fs::write(
        &verify_path,
        migration_verify_assertion(&module_repo_url, current_version, next_version),
    )?;
    Ok(())
}

fn build_summary(
    repo_path: &Path,
    package_name: String,
    plan: &ReleasePlan,
    push: bool,
    scripts: Vec<String>,
) -> ReleaseSummary {
    ReleaseSummary {
        repo_path: repo_path.to_path_buf(),
        package_name,
        current_version: plan.current_version.clone(),
        next_version: Some(plan.next_version.clone()),
        tag_name: Some(format!("v{}", plan.next_version)),
        push,
        resume_existing_version: false,
        renamed_paths: plan.rename_plan.clone(),
        scripts,
        mode: "release",
        migration_diff: None,
    }
}

fn push_release(repo_path: &Path, tag_name: &str) -> Result<()> {
    let tag_ref = format!("refs/tags/{tag_name}");
    run_command(
        "git",
        ["push", "--atomic", "origin", "HEAD", tag_ref.as_str()],
        repo_path,
        false,
    )?;
    Ok(())
}

fn json_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

fn emit_text(summary: &ReleaseSummary) {
    println!("release:       {}", summary.mode);
    println!("repo:          {}", summary.repo_path.display());
    println!("package:       {}", summary.package_name);
    println!("current:       {}", summary.current_version);
    println!(
        "next:          {}",
        summary.next_version.as_deref().unwrap_or("none")
    );
    println!(
        "tag:           {}",
        summary.tag_name.as_deref().unwrap_or("none")
    );
    println!("push:          {}", summary.push);
    println!("resume:        {}", summary.resume_existing_version);
    println!("renamed paths: {}", summary.renamed_paths.len());
    println!("scripts:       {}", summary.scripts.join(", "));
    if let Some(migration_diff) = &summary.migration_diff {
        println!("migration:     {migration_diff}");
    }
}
