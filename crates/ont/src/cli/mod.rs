use std::io::Write;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use serde::Serialize;

mod completions;
mod corpus;
mod diff;
mod prepare_package;
mod release;
mod validate_migration;
mod validate_package;
mod version;

/// Top-level CLI definition for `ont`.
#[derive(Parser, Debug, Clone)]
#[command(
    name = "ont",
    version,
    about = "Vibe Machine ontology control surface",
    long_about = "ont is the unified CLI/TUI for working with Vibe Machine ontology corpora, packages, and runtime state.\n\nThe binary delegates to the durable `vibe-ontology` library for all corpus operations."
)]
pub struct Cli {
    /// Output format for scripted commands.
    #[arg(long, global = true, value_enum, env = "ONT_FORMAT")]
    pub format: Option<Format>,

    /// Increase log verbosity (-v, -vv).
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Suppress informational stderr output.
    #[arg(short, long, global = true)]
    pub quiet: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum Command {
    /// Generate shell completions for the supplied shell.
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    /// Manage executable ontology corpora.
    Corpus(corpus::CorpusArgs),
    /// Generate a migration diff between two package versions.
    Diff(diff::DiffArgs),
    /// Prepare an ontology package's executable apply units (splits large writes).
    PreparePackage(prepare_package::PreparePackageArgs),
    /// Validate or publish an ontology package release.
    Release(release::ReleaseArgs),
    /// Launch the interactive TUI.
    Tui,
    /// Validate an ontology package's migration contract.
    ValidateMigration(validate_migration::ValidateMigrationArgs),
    /// Validate an ontology package contract.
    ValidatePackage(validate_package::ValidatePackageArgs),
    /// Print version information.
    Version,
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Json,
    Text,
}

impl Format {
    pub fn resolve(opt: Option<Format>) -> Format {
        opt.unwrap_or(Format::Json)
    }
}

pub async fn run(cli: Cli) -> Result<()> {
    let Some(command) = cli.command.clone() else {
        let mut cmd = Cli::command();
        cmd.print_help()?;
        println!();
        return Ok(());
    };

    match command {
        Command::Completions { shell } => completions::run(shell),
        Command::Corpus(args) => corpus::run(&cli, &args).await,
        Command::Diff(args) => diff::run(&cli, &args),
        Command::PreparePackage(args) => prepare_package::run(&cli, &args),
        Command::Release(args) => release::run(&cli, &args),
        Command::Tui => crate::tui::run(&cli).await,
        Command::ValidateMigration(args) => validate_migration::run(&cli, &args),
        Command::ValidatePackage(args) => validate_package::run(&cli, &args),
        Command::Version => version::run(&cli),
    }
}

pub(crate) fn emit_json<T: Serialize>(value: &T) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, value)?;
    writeln!(stdout)?;
    Ok(())
}
