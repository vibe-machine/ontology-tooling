use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use clap::Args;
use serde::Serialize;
use vibe_ontology::throwaway_typedb::ThrowawayTypeDb;
use vibe_ontology::upgrade_check::{check_upgrade, CaseOutcome, CaseStatus, CheckScope};

use super::qualify::QUALIFYING_ENV;
use super::{emit_json, Cli, Format};

/// Upgrade to the candidate the way the app does, from every released
/// version and through one database that lived through every release.
#[derive(Args, Debug, Clone)]
pub struct CheckUpgradeArgs {
    /// Path to the ontology repository (the candidate's working tree).
    #[arg(long)]
    pub repo: PathBuf,

    /// Only from the previous release, plus the lived-through database.
    #[arg(long)]
    pub quick: bool,

    /// TypeDB executable to start (defaults to TYPEDB_BIN, then `typedb` on PATH).
    #[arg(long)]
    pub typedb_bin: Option<PathBuf>,

    /// Seconds one unit may take before the case fails.
    #[arg(long, default_value_t = 120)]
    pub unit_timeout: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Report {
    repo_path: PathBuf,
    status: &'static str,
    total_millis: u128,
    cases: Vec<CaseOutcome>,
}

pub async fn run(cli: &Cli, args: &CheckUpgradeArgs) -> Result<()> {
    if std::env::var_os(QUALIFYING_ENV).is_some() {
        eprintln!("check-upgrade: skipped here; `ont qualify` runs it as its own stage");
        return Ok(());
    }
    let repo_path = std::fs::canonicalize(&args.repo)
        .with_context(|| format!("resolve repository path {}", args.repo.display()))?;
    let started = Instant::now();
    let server = ThrowawayTypeDb::start(args.typedb_bin.as_deref()).context("start TypeDB")?;
    let quiet = cli.quiet;
    let mut progress = |line: &str| {
        if !quiet {
            eprintln!("{line}");
        }
    };
    let cases = check_upgrade(
        &server.target(),
        &repo_path,
        if args.quick {
            CheckScope::Quick
        } else {
            CheckScope::Full
        },
        Duration::from_secs(args.unit_timeout),
        &mut progress,
    )
    .await
    .context("check upgrade")?;
    let failed = cases
        .iter()
        .filter(|case| matches!(case.status, CaseStatus::Failed(_)))
        .count();
    let report = Report {
        repo_path,
        status: if failed == 0 { "ok" } else { "failed" },
        total_millis: started.elapsed().as_millis(),
        cases,
    };
    if !quiet {
        eprintln!(
            "total: {:.1}s, {} cases, {failed} failed",
            report.total_millis as f64 / 1000.0,
            report.cases.len()
        );
    }
    match Format::resolve(cli.format) {
        Format::Json => emit_json(&report)?,
        Format::Text => {}
    }
    if failed > 0 {
        bail!("{failed} upgrade case(s) failed");
    }
    Ok(())
}
