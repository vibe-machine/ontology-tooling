use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};

mod completions;
mod corpus;
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
    /// Launch the interactive TUI.
    Tui,
    /// Manage executable ontology corpora.
    Corpus(corpus::CorpusArgs),
    /// Print version information.
    Version,
    /// Generate shell completions for the supplied shell.
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
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
        Command::Tui => crate::tui::run(&cli).await,
        Command::Corpus(args) => corpus::run(&cli, &args).await,
        Command::Version => version::run(&cli),
        Command::Completions { shell } => completions::run(shell),
    }
}
