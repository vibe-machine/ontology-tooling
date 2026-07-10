# Rust Workspace

`ontology-tooling` is a hybrid repo. The historical Node CLIs (`ontology-release`,
`ontology-validate-package`) own npm package release orchestration, and stay.
The Rust workspace at the repo root owns the durable runtime: corpus model,
ontology operations, and the unified CLI/TUI.

## Layout

```
Cargo.toml                     # workspace root
crates/
  vibe-ontology/               # library — durable, embeddable, no CLI/TUI deps
    src/
      lib.rs
      error.rs
      corpus/
        mod.rs                 # public surface + ItemFilter
        model.rs               # CorpusManifest, CorpusItem, enums
        discover.rs            # filesystem discovery
        validate.rs            # shape validation
        export.rs              # prompt-export selection + artifact shape
    tests/corpus.rs
  ont/                         # binary — depends on vibe-ontology + clap + ratatui
    src/
      main.rs                  # tokio runtime, ExitCode dispatch
      logging.rs               # tracing-subscriber to stderr
      cli/
        mod.rs                 # Cli, Command, Format
        corpus.rs              # `ont corpus list|validate`
        version.rs             # `ont version`
        completions.rs         # `ont completions <shell>`
      tui/
        mod.rs                 # entry point: install hooks, drive event loop
        terminal.rs            # TerminalSession (RAII raw mode + alt screen)
        panic.rs               # panic hook restores terminal
        event.rs               # crossterm + ticks + Ctrl-C
        app.rs                 # AppState + update + draw
    tests/cli.rs               # assert_cmd integration tests
```

## Dependency direction

```
ont (bin)
  └── vibe-ontology (lib)         ← OneApp / Lingo / other products depend on this
  └── clap, ratatui, crossterm    ← UI deps live only in the bin
```

`vibe-ontology` is the contract for external consumers. It exposes no CLI or
TUI types. Anyone needing corpus discovery/validation/export from another
product takes a dependency on `vibe-ontology` directly and links no terminal
machinery.

## Lints and safety

The workspace `Cargo.toml` sets `unsafe_code = "forbid"` and CI gates
`cargo clippy --workspace --all-targets -- -D warnings`. Don't reach for
`#[allow]` without a comment explaining why.

## Common commands

```bash
mise run build         # cargo build --workspace
mise run cargo-test    # cargo test --workspace
mise run lint          # cargo clippy --workspace --all-targets -- -D warnings
mise run check         # smoke-tests `ont --help` paths
mise run test          # cargo test --workspace
```

`./target/debug` is on `$PATH` via `mise.toml`, so `ont` is callable from any
mise shell once `mise run build` has run.
