use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Args;
use serde::Serialize;
use vibe_ontology::migration_contract::validate_migration_contract;

use super::{emit_json, Cli, Format};

#[derive(Args, Debug, Clone)]
pub struct ValidateMigrationArgs {
    /// Path to the ontology repository whose migration contract to validate.
    #[arg(long)]
    pub repo: PathBuf,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ValidateMigrationReport<'a> {
    repo_path: &'a Path,
    status: &'static str,
}

pub fn run(cli: &Cli, args: &ValidateMigrationArgs) -> Result<()> {
    let repo_path = std::fs::canonicalize(&args.repo)
        .with_context(|| format!("resolve repository path {}", args.repo.display()))?;
    validate_migration_contract(&repo_path)
        .with_context(|| format!("validate migration contract at {}", repo_path.display()))?;

    let report = ValidateMigrationReport {
        repo_path: &repo_path,
        status: "ok",
    };

    match Format::resolve(cli.format) {
        Format::Json => emit_json(&report)?,
        Format::Text => emit_text(&report),
    }
    Ok(())
}

fn emit_text(report: &ValidateMigrationReport<'_>) {
    println!("validate migration: [{}]", report.status);
    println!("repo:               {}", report.repo_path.display());
}
