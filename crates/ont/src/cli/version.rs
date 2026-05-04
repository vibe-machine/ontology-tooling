use anyhow::Result;
use serde::Serialize;

use super::{Cli, Format};

#[derive(Serialize)]
struct VersionInfo {
    name: &'static str,
    version: &'static str,
    library: LibraryInfo,
}

#[derive(Serialize)]
struct LibraryInfo {
    name: &'static str,
    version: &'static str,
}

pub fn run(cli: &Cli) -> Result<()> {
    let info = VersionInfo {
        name: env!("CARGO_PKG_NAME"),
        version: env!("CARGO_PKG_VERSION"),
        library: LibraryInfo {
            name: "vibe-ontology",
            version: env!("CARGO_PKG_VERSION"),
        },
    };

    match Format::resolve(cli.format) {
        Format::Json => {
            let mut stdout = std::io::stdout().lock();
            serde_json::to_writer_pretty(&mut stdout, &info)?;
            use std::io::Write;
            writeln!(stdout)?;
        }
        Format::Text => {
            println!("{} {}", info.name, info.version);
            println!("library: {} {}", info.library.name, info.library.version);
        }
    }
    Ok(())
}
