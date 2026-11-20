use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;
use serde::Serialize;
use vibe_ontology::executable_package::{
    render_contextual_apply_units, sha256_hex, APPLY_UNITS_ROOT,
};

use super::{emit_json, Cli, Format};

/// Split one TypeQL write file into apply units for a package generator.
#[derive(Args, Debug, Clone)]
pub struct SplitApplyUnitsArgs {
    /// Directory the source is read from and the apply units are written under
    /// (the package root, or a generator's --out-dir).
    #[arg(long)]
    pub root: PathBuf,

    /// Root-relative path of the TypeQL file to split, e.g. data/gistCore-schema-docs.tql.
    #[arg(long)]
    pub source: String,

    /// Manifest path to name in each unit's header.
    #[arg(long)]
    pub manifest: Option<String>,

    /// Upstream commit to name in each unit's header.
    #[arg(long)]
    pub upstream_commit: Option<String>,
}

#[derive(Serialize)]
struct Artifact {
    kind: &'static str,
    path: String,
    sha256: String,
}

#[derive(Serialize)]
struct SplitApplyUnitsReport {
    source: String,
    paths: Vec<String>,
    artifacts: Vec<Artifact>,
}

pub fn run(cli: &Cli, args: &SplitApplyUnitsArgs) -> Result<()> {
    let source_file = args.root.join(&args.source);
    let text = std::fs::read_to_string(&source_file)
        .with_context(|| format!("read {}", source_file.display()))?;
    let units = render_contextual_apply_units(
        &args.source,
        &text,
        args.manifest.as_deref(),
        args.upstream_commit.as_deref(),
    )
    .with_context(|| format!("split {}", args.source))?;

    // Replace this source's previous units only; other sources' units stay.
    let stem = args.source.strip_suffix(".tql").unwrap_or(&args.source);
    let unit_dir = args.root.join(APPLY_UNITS_ROOT).join(stem);
    match std::fs::remove_dir_all(&unit_dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("remove {}", unit_dir.display()));
        }
    }
    std::fs::create_dir_all(&unit_dir).with_context(|| format!("create {}", unit_dir.display()))?;

    let mut report = SplitApplyUnitsReport {
        source: args.source.clone(),
        paths: Vec::with_capacity(units.len()),
        artifacts: Vec::with_capacity(units.len()),
    };
    for unit in units {
        let target = args.root.join(&unit.path);
        std::fs::write(&target, &unit.text)
            .with_context(|| format!("write {}", target.display()))?;
        report.artifacts.push(Artifact {
            kind: "apply-unit",
            path: unit.path.clone(),
            sha256: sha256_hex(&unit.text),
        });
        report.paths.push(unit.path);
    }

    match Format::resolve(cli.format) {
        Format::Json => emit_json(&report)?,
        Format::Text => {
            println!(
                "split {}: {} apply units",
                report.source,
                report.paths.len()
            );
            for path in &report.paths {
                println!("  {path}");
            }
        }
    }
    Ok(())
}
