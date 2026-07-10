use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Args;
use serde::Serialize;
use vibe_ontology::executable_package::prepare_executable_package;

use super::{emit_json, Cli, Format};

#[derive(Args, Debug, Clone)]
pub struct PreparePackageArgs {
    /// Path to the ontology repository whose executable package to prepare.
    #[arg(long)]
    pub repo: PathBuf,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PreparePackageReport<'a> {
    repo_path: &'a Path,
    package_name: &'a str,
    version: &'a str,
    status: &'static str,
}

pub fn run(cli: &Cli, args: &PreparePackageArgs) -> Result<()> {
    let repo_path = std::fs::canonicalize(&args.repo)
        .with_context(|| format!("resolve repository path {}", args.repo.display()))?;
    let package = prepare_executable_package(&repo_path, None)
        .with_context(|| format!("prepare executable package at {}", repo_path.display()))?;
    let package_name = package
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let version = package
        .get("version")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");

    let report = PreparePackageReport {
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

fn emit_text(report: &PreparePackageReport<'_>) {
    println!(
        "prepare package: {} v{}  [{}]",
        report.package_name, report.version, report.status
    );
    println!("repo:            {}", report.repo_path.display());
}
