//! `ont qualify`: the one command that says whether a package candidate is fit
//! to release. `ont release` runs it on the bumped tree before it commits.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use clap::Args;
use serde_json::Value;
use vibe_ontology::bootstrap_uniqueness::validate_bootstrap_uniqueness;
use vibe_ontology::competency_questions::{
    read_declaration, resolve_package_order, CompetencyDeclaration,
};
use vibe_ontology::package_release::release_scripts_to_run;
use vibe_ontology::package_validator::validate_package_contract;
use vibe_ontology::throwaway_typedb::ThrowawayTypeDb;
use vibe_ontology::upgrade_check::{check_upgrade, CaseStatus, CheckScope};

use super::check_questions::run_and_record;
use super::Cli;

/// Set while `ont qualify` runs a package's own tests, so a
/// `test:typedb-migration` script that calls `ont check-upgrade` does not run
/// the upgrade test a second time.
pub const QUALIFYING_ENV: &str = "ONT_QUALIFYING";

/// Decide whether a package candidate is fit to release.
#[derive(Args, Debug, Clone)]
pub struct QualifyArgs {
    /// Path to the ontology repository (the candidate's working tree).
    #[arg(long)]
    pub repo: PathBuf,

    /// Upgrade only from the previous release and through the lived-through
    /// database, not from every release (for iteration; `ont release` always
    /// runs the full test).
    #[arg(long)]
    pub quick: bool,

    /// TypeDB executable to start (defaults to TYPEDB_BIN, then `typedb` on PATH).
    #[arg(long)]
    pub typedb_bin: Option<PathBuf>,
}

/// What [`qualify`] runs.
#[derive(Debug, Clone)]
pub struct QualifyOptions {
    pub quick: bool,
    /// Run the competency questions and record competency/last-run.json
    /// (`false` when validating a committed release whose run record was
    /// already checked).
    pub run_questions: bool,
    pub typedb_bin: Option<PathBuf>,
}

pub async fn run(_cli: &Cli, args: &QualifyArgs) -> Result<()> {
    let repo_path = std::fs::canonicalize(&args.repo)
        .with_context(|| format!("resolve repository path {}", args.repo.display()))?;
    qualify(
        &repo_path,
        &QualifyOptions {
            quick: args.quick,
            run_questions: true,
            typedb_bin: args.typedb_bin.clone(),
        },
    )
    .await
}

fn line(stage: &str, started: Instant, outcome: &str) {
    eprintln!(
        "qualify: {stage:<16} {outcome} ({:.1}s)",
        started.elapsed().as_secs_f64()
    );
}

/// Runs the stages in order, stopping at the first failure: validate-package,
/// check-questions, the package's own tests, then the upgrade test from every
/// released version and through the lived-through database. Prints one line
/// per stage and ends with PASS or FAIL.
pub async fn qualify(repo_path: &Path, options: &QualifyOptions) -> Result<()> {
    let total = Instant::now();
    let result = run_stages(repo_path, options).await;
    match &result {
        Ok(()) => {
            eprintln!("qualify: PASS ({:.1}s)", total.elapsed().as_secs_f64());
            eprintln!(
                "qualify: one gate remains, which needs the app's code: the app's own install, \
                 upgrade from each earlier release and lived-through database. In OneWorkspace \
                 run:\n  {}\n(`--quick` while iterating; `--install-only` and `--time-limit <seconds>` \
                 are available. A release is fit only when it ends PASS.)",
                app_check_command(repo_path)
            );
        }
        Err(error) => eprintln!(
            "qualify: FAIL ({:.1}s): {error:#}",
            total.elapsed().as_secs_f64()
        ),
    }
    result
}

/// `mise run check-package-release <candidate> <dependencies…>`.
pub fn app_check_command(repo_path: &Path) -> String {
    let mut parts = vec![
        "mise run check-package-release".to_string(),
        repo_path.display().to_string(),
    ];
    if let Ok(ordered) = resolve_package_order(repo_path) {
        parts.extend(
            ordered
                .iter()
                .take(ordered.len().saturating_sub(1))
                .map(|package| package.root.display().to_string()),
        );
    }
    parts.join(" ")
}

async fn run_stages(repo_path: &Path, options: &QualifyOptions) -> Result<()> {
    let started = Instant::now();
    let package = validate_package_contract(repo_path)
        .and_then(|package| validate_bootstrap_uniqueness(repo_path).map(|()| package))
        .context("validate-package");
    let package: Value = match package {
        Ok(package) => {
            line("validate-package", started, "ok");
            package
        }
        Err(error) => {
            line("validate-package", started, "FAIL");
            return Err(error);
        }
    };

    let started = Instant::now();
    match read_declaration(repo_path, &package)? {
        CompetencyDeclaration::Waived { reason } => {
            line("check-questions", started, &format!("waived: {reason}"));
        }
        CompetencyDeclaration::Declared { .. } if !options.run_questions => {
            line("check-questions", started, "run record already verified");
        }
        CompetencyDeclaration::Declared { path, file } => {
            let (run, _) = run_and_record(
                repo_path,
                &package,
                &path,
                &file,
                options.typedb_bin.as_deref(),
                true,
            )
            .await
            .context("check-questions")?;
            let failed = run
                .outcomes
                .iter()
                .filter(|outcome| !outcome.passed())
                .count();
            if failed > 0 {
                line("check-questions", started, "FAIL");
                bail!(
                    "{failed} of {} competency questions failed",
                    run.outcomes.len()
                );
            }
            if let Err(problem) = &run.installed {
                line("check-questions", started, "FAIL");
                bail!("the candidate installs its questions wrongly: {problem}");
            }
            line(
                "check-questions",
                started,
                &format!("ok ({} questions)", run.outcomes.len()),
            );
        }
    }

    let started = Instant::now();
    let has_test = package
        .pointer("/scripts/test")
        .and_then(Value::as_str)
        .is_some_and(|script| !script.trim().is_empty());
    let scripts: Vec<Vec<String>> = if has_test {
        vec![vec!["test".to_string()]]
    } else {
        release_scripts_to_run(&package)
            .into_iter()
            .filter(|script| {
                script != "refresh:package-contract" && script != "test:typedb-migration"
            })
            .map(|script| vec!["run".to_string(), script])
            .collect()
    };
    for arguments in &scripts {
        let status = Command::new("npm")
            .args(arguments)
            .current_dir(repo_path)
            .env(QUALIFYING_ENV, "1")
            .status()
            .with_context(|| format!("run npm {}", arguments.join(" ")))?;
        if !status.success() {
            line("package tests", started, "FAIL");
            bail!("npm {} exited with {status}", arguments.join(" "));
        }
    }
    line("package tests", started, "ok");

    let started = Instant::now();
    let server = ThrowawayTypeDb::start(options.typedb_bin.as_deref()).context("start TypeDB")?;
    let mut progress = |text: &str| eprintln!("  {text}");
    let outcomes = check_upgrade(
        &server.target(),
        repo_path,
        if options.quick {
            CheckScope::Quick
        } else {
            CheckScope::Full
        },
        Duration::from_secs(120),
        &mut progress,
    )
    .await
    .context("check-upgrade")?;
    let failed: Vec<String> = outcomes
        .iter()
        .filter_map(|outcome| match &outcome.status {
            CaseStatus::Failed(error) => Some(format!("{}: {error}", outcome.name)),
            _ => None,
        })
        .collect();
    let unusable = outcomes
        .iter()
        .filter(|outcome| matches!(outcome.status, CaseStatus::UnusableBase(_)))
        .count();
    if !failed.is_empty() {
        line("check-upgrade", started, "FAIL");
        bail!("upgrade failed {}", failed.join("; "));
    }
    line(
        "check-upgrade",
        started,
        &format!(
            "ok ({} cases, {unusable} could not be used as a base)",
            outcomes.len()
        ),
    );
    Ok(())
}
