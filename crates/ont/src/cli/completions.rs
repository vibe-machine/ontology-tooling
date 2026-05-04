use anyhow::Result;
use clap::CommandFactory;
use clap_complete::{generate, Shell};

use super::Cli;

pub fn run(shell: Shell) -> Result<()> {
    let mut cmd = Cli::command();
    let bin_name = cmd.get_name().to_string();
    let mut stdout = std::io::stdout().lock();
    generate(shell, &mut cmd, bin_name, &mut stdout);
    Ok(())
}
