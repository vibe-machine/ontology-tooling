use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Args;
use serde::Serialize;
use vibe_ontology::migration_diff::generate_migration_diff;

use super::{Cli, Format};

#[derive(Args, Debug, Clone)]
pub struct DiffArgs {
    /// Path to the ontology repository to diff.
    #[arg(long)]
    pub repo: PathBuf,

    /// Released version to compare from.
    #[arg(long)]
    pub from: String,

    /// Target version to generate the migration for.
    #[arg(long)]
    pub to: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DiffReport<'a> {
    repo_path: &'a Path,
    from: &'a str,
    to: &'a str,
    migration_path: Option<String>,
}

pub fn run(cli: &Cli, args: &DiffArgs) -> Result<()> {
    let repo_path = std::fs::canonicalize(&args.repo)
        .with_context(|| format!("resolve repository path {}", args.repo.display()))?;
    let migration_path =
        generate_migration_diff(&repo_path, &args.from, &args.to).with_context(|| {
            format!(
                "generate migration diff from {} to {} at {}",
                args.from,
                args.to,
                repo_path.display()
            )
        })?;

    let report = DiffReport {
        repo_path: &repo_path,
        from: &args.from,
        to: &args.to,
        migration_path,
    };

    match Format::resolve(cli.format) {
        Format::Json => emit_json(&report)?,
        Format::Text => emit_text(&report),
    }
    Ok(())
}

fn emit_json<T: Serialize>(value: &T) -> Result<()> {
    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, value)?;
    writeln!(stdout)?;
    Ok(())
}

fn emit_text(report: &DiffReport<'_>) {
    println!("migration diff: {} -> {}", report.from, report.to);
    println!("repo:           {}", report.repo_path.display());
    println!(
        "migration:      {}",
        report.migration_path.as_deref().unwrap_or("none")
    );
}
