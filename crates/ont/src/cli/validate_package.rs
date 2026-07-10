use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Args;
use serde::Serialize;
use vibe_ontology::package_validator::validate_package_contract;

use super::{emit_json, Cli, Format};

#[derive(Args, Debug, Clone)]
pub struct ValidatePackageArgs {
    /// Path to the ontology repository whose package contract to validate.
    #[arg(long)]
    pub repo: PathBuf,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ValidatePackageReport<'a> {
    repo_path: &'a Path,
    package_name: &'a str,
    version: &'a str,
    status: &'static str,
}

pub fn run(cli: &Cli, args: &ValidatePackageArgs) -> Result<()> {
    let repo_path = std::fs::canonicalize(&args.repo)
        .with_context(|| format!("resolve repository path {}", args.repo.display()))?;
    let package = validate_package_contract(&repo_path)
        .with_context(|| format!("validate package contract at {}", repo_path.display()))?;
    let package_name = package
        .get("name")
        .and_then(serde_json::Value::as_str)
        .context("validated package is missing string field 'name'")?;
    let version = package
        .get("version")
        .and_then(serde_json::Value::as_str)
        .context("validated package is missing string field 'version'")?;

    let report = ValidatePackageReport {
        repo_path: &repo_path,
        package_name,
        version,
        status: "ok",
    };

    match Format::resolve(cli.format) {
        Format::Json => emit_json(&report)?,
        Format::Text => emit_text(&report),
    }
    Ok(())
}

fn emit_text(report: &ValidatePackageReport<'_>) {
    println!(
        "validate package: {} v{}  [{}]",
        report.package_name, report.version, report.status
    );
    println!("repo:             {}", report.repo_path.display());
}
