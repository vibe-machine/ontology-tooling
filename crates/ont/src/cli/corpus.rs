use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use serde::Serialize;
use vibe_ontology::corpus::{
    discover_corpus, Corpus, CorpusItem, ItemCategory, ItemFilter,
};

use super::{Cli, Format};

#[derive(Args, Debug, Clone)]
pub struct CorpusArgs {
    #[command(subcommand)]
    pub command: CorpusCommand,
}

#[derive(Subcommand, Debug, Clone)]
pub enum CorpusCommand {
    /// Discover and list corpus items in an ontology repo.
    List {
        /// Path to the ontology repository whose `corpus/` directory to inspect.
        #[arg(long)]
        repo: PathBuf,
        /// Restrict to items whose `ontology_tags` contain this tag.
        #[arg(long)]
        tag: Option<String>,
        /// Restrict to a single item by id.
        #[arg(long)]
        item: Option<String>,
    },
    /// Validate corpus shape (manifest, fixtures, items) without query execution.
    Validate {
        /// Path to the ontology repository whose `corpus/` directory to validate.
        #[arg(long)]
        repo: PathBuf,
    },
}

pub async fn run(cli: &Cli, args: &CorpusArgs) -> Result<()> {
    match &args.command {
        CorpusCommand::List { repo, tag, item } => {
            list(cli, repo, tag.as_deref(), item.as_deref())
        }
        CorpusCommand::Validate { repo } => validate(cli, repo),
    }
}

#[derive(Serialize)]
struct ManifestSummary {
    ontology_package: String,
    version: u32,
    fixture_count: usize,
}

#[derive(Serialize)]
struct ListReport<'a> {
    command: &'static str,
    repo: &'a Path,
    manifest: ManifestSummary,
    filters: Filters<'a>,
    matched: usize,
    total: usize,
    items: Vec<ItemSummary<'a>>,
}

#[derive(Serialize)]
struct ValidateReport<'a> {
    command: &'static str,
    repo: &'a Path,
    manifest: ManifestSummary,
    counts: Counts,
    status: &'static str,
}

#[derive(Serialize)]
struct Counts {
    total: usize,
    known_good: usize,
    negative: usize,
    fixtures: usize,
}

#[derive(Serialize)]
struct Filters<'a> {
    tag: Option<&'a str>,
    item: Option<&'a str>,
}

#[derive(Serialize)]
struct ItemSummary<'a> {
    id: &'a str,
    title: &'a str,
    category: ItemCategory,
    query_kind: &'a vibe_ontology::corpus::QueryKind,
    ontology_tags: &'a [String],
    prompt_export: &'a vibe_ontology::corpus::PromptExport,
    source: &'a str,
}

fn summarize_manifest(corpus: &Corpus) -> ManifestSummary {
    ManifestSummary {
        ontology_package: corpus.manifest.ontology_package.clone(),
        version: corpus.manifest.version,
        fixture_count: corpus.manifest.fixtures.len(),
    }
}

fn list(cli: &Cli, repo: &Path, tag: Option<&str>, item_id: Option<&str>) -> Result<()> {
    tracing::info!(repo = %repo.display(), "discovering corpus");
    let corpus = discover_corpus(repo)
        .with_context(|| format!("discover corpus at {}", repo.display()))?;

    let filter = ItemFilter {
        tag: tag.map(|s| s.to_string()),
        item_id: item_id.map(|s| s.to_string()),
        category: None,
    };
    let matched: Vec<&CorpusItem> =
        corpus.items.iter().filter(|i| filter.matches(i)).collect();

    let report = ListReport {
        command: "corpus.list",
        repo,
        manifest: summarize_manifest(&corpus),
        filters: Filters { tag, item: item_id },
        matched: matched.len(),
        total: corpus.items.len(),
        items: matched.iter().map(|i| item_summary(i)).collect(),
    };

    match Format::resolve(cli.format) {
        Format::Json => emit_json(&report)?,
        Format::Text => emit_list_text(&report),
    }
    Ok(())
}

fn validate(cli: &Cli, repo: &Path) -> Result<()> {
    tracing::info!(repo = %repo.display(), "validating corpus shape");
    let corpus = discover_corpus(repo)
        .with_context(|| format!("validate corpus at {}", repo.display()))?;

    let known_good = corpus
        .items
        .iter()
        .filter(|i| matches!(i.category, ItemCategory::KnownGood))
        .count();
    let negative = corpus.items.len() - known_good;

    let report = ValidateReport {
        command: "corpus.validate",
        repo,
        manifest: summarize_manifest(&corpus),
        counts: Counts {
            total: corpus.items.len(),
            known_good,
            negative,
            fixtures: corpus.manifest.fixtures.len(),
        },
        status: "ok",
    };

    match Format::resolve(cli.format) {
        Format::Json => emit_json(&report)?,
        Format::Text => emit_validate_text(&report),
    }
    Ok(())
}

fn item_summary(item: &CorpusItem) -> ItemSummary<'_> {
    ItemSummary {
        id: &item.id,
        title: &item.title,
        category: item.category,
        query_kind: &item.query_kind,
        ontology_tags: &item.ontology_tags,
        prompt_export: &item.prompt_export,
        source: &item.source,
    }
}

fn emit_json<T: Serialize>(value: &T) -> Result<()> {
    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, value)?;
    writeln!(stdout)?;
    Ok(())
}

fn emit_list_text(report: &ListReport<'_>) {
    println!(
        "corpus list: {} v{}",
        report.manifest.ontology_package, report.manifest.version
    );
    println!("repo:        {}", report.repo.display());
    println!("matched:     {} / {}", report.matched, report.total);
    if report.filters.tag.is_some() || report.filters.item.is_some() {
        println!(
            "filters:     tag={} item={}",
            report.filters.tag.unwrap_or("*"),
            report.filters.item.unwrap_or("*"),
        );
    }
    for item in &report.items {
        println!(
            "  {:<32} {:<10} {:<8} {}",
            item.id,
            format!("{:?}", item.category).to_lowercase(),
            format!("{:?}", item.query_kind).to_lowercase(),
            item.title,
        );
    }
}

fn emit_validate_text(report: &ValidateReport<'_>) {
    println!(
        "corpus validate: {} v{}  [{}]",
        report.manifest.ontology_package, report.manifest.version, report.status
    );
    println!("repo:            {}", report.repo.display());
    println!(
        "items:           total={} known_good={} negative={}",
        report.counts.total, report.counts.known_good, report.counts.negative
    );
    println!("fixtures:        {}", report.counts.fixtures);
}
