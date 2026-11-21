use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Args;
use serde::Serialize;
use vibe_ontology::competency_questions::{read_declaration, CompetencyDeclaration};
use vibe_ontology::package_validator::validate_package_contract;
use vibe_ontology::release_integrity::validate_released;

use super::{emit_json, Cli, Format};

#[derive(Args, Debug, Clone)]
pub struct ValidatePackageArgs {
    /// Path to the ontology repository whose package contract to validate.
    #[arg(long)]
    pub repo: PathBuf,

    /// Also check that the tree is publishable as the version it declares:
    /// nothing shipped changed since tag v<version>, manifest hashes match the
    /// files, competency/last-run.json names this version, and the bump from
    /// the previous tag is large enough for the schema change. Run by the
    /// pre-push hook and by `ont release` after it tags.
    #[arg(long)]
    pub released: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ValidatePackageReport<'a> {
    repo_path: &'a Path,
    package_name: &'a str,
    version: &'a str,
    status: &'static str,
    released: bool,
}

pub fn run(cli: &Cli, args: &ValidatePackageArgs) -> Result<()> {
    let repo_path = std::fs::canonicalize(&args.repo)
        .with_context(|| format!("resolve repository path {}", args.repo.display()))?;
    let package = validate_package_contract(&repo_path)
        .with_context(|| format!("validate package contract at {}", repo_path.display()))?;
    if args.released {
        validate_released(&repo_path, &package)
            .with_context(|| format!("validate released package at {}", repo_path.display()))?;
    }
    let package_name = package
        .get("name")
        .and_then(serde_json::Value::as_str)
        .context("validated package is missing string field 'name'")?;
    let version = package
        .get("version")
        .and_then(serde_json::Value::as_str)
        .context("validated package is missing string field 'version'")?;

    if let CompetencyDeclaration::Waived { reason } = read_declaration(&repo_path, &package)? {
        if !cli.quiet {
            eprintln!("notice: {package_name} waives competency questions: {reason}");
        }
    }

    let report = ValidatePackageReport {
        repo_path: &repo_path,
        package_name,
        version,
        status: "ok",
        released: args.released,
    };

    match Format::resolve(cli.format) {
        Format::Json => emit_json(&report)?,
        Format::Text => emit_text(&report),
    }
    Ok(())
}

fn emit_text(report: &ValidatePackageReport<'_>) {
    println!(
        "validate package: {} v{}  [{}{}]",
        report.package_name,
        report.version,
        report.status,
        if report.released { ", released" } else { "" }
    );
    println!("repo:             {}", report.repo_path.display());
}
