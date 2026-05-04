use std::process::ExitCode;

use clap::Parser;

mod cli;
mod logging;
mod tui;

fn main() -> ExitCode {
    let cli = cli::Cli::parse();

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(error) => {
            eprintln!("ont: failed to start async runtime: {error}");
            return ExitCode::from(1);
        }
    };

    let result = runtime.block_on(async {
        let _logging = logging::init(&cli)?;
        cli::run(cli).await
    });

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(error = %error, "ont command failed");
            eprintln!("ont: {error:#}");
            ExitCode::from(1)
        }
    }
}
