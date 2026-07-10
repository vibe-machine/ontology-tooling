# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Purpose

ontology-tooling is the shared tooling hub for the `collection-vibe-machine` project. It is a **single Rust workspace** (the former Node release tooling has been fully consolidated into it):

1. **`vibe-ontology`** — the durable, embeddable library: corpus model, schema/migration-contract validation, package-release planning, migration diffing, and TypeDB schema application. Other Vibe Machine products (OneApp, Lingo) embed it.
2. **`ont`** — the CLI/TUI binary over that library. Automates ontology-repo releases (`ont release`), package/migration validation, migration diffs, and corpus inspection (local-first, not CI/CD).

`bin/ontology-release` and `bin/ontology-validate-package` are stable cross-repo entrypoints (consumer ontology repos call them by path from their `package.json`); each is a thin shim that builds `ont` on first use and delegates to it.

## Commands

```bash
# Build / test / lint
mise run build                                            # cargo build --workspace
mise run test                                             # cargo test --workspace
mise run lint                                             # cargo clippy --workspace -D warnings
mise run check                                            # smoke `ont --help` paths

# Release (all backed by `ont release`)
mise run release-check -- ../ontology-repo                # Validate repo without mutation
mise run release-dry-run -- ../ontology-repo [bump]       # Preview next release
mise run release -- --repo ../ontology-repo --bump patch  # Execute release

# ont CLI (after build; ./target/debug on $PATH via mise.toml)
ont --help                                                # Top-level CLI help
ont validate-package --repo ../ontology-gist              # Package-contract validation
ont validate-migration --repo ../ontology-gist            # Migration-contract validation
ont diff --repo ../ontology-gist --from 1.0.0 --to 1.0.1  # Migration diff
ont release --repo ../ontology-gist --bump patch          # Execute a release
ont corpus list --repo ../ontology-gist                   # List corpus items
ont corpus validate --repo ../ontology-gist               # Shape-validate a corpus
ont tui                                                   # Launch interactive TUI
```

## Architecture

```
Cargo.toml                                → Workspace root
crates/vibe-ontology/                     → Library (durable, embeddable, no CLI/TUI deps)
  src/corpus.rs                           → Corpus model & discovery
  src/version.rs                          → Semver parsing, bumping, version resolution
  src/package_validator.rs                → Package-contract validation
  src/migration_contract.rs               → Migration-contract validation
  src/migration_diff.rs                   → Migration diffing
  src/executable_package.rs               → Executable-package preparation
  src/bootstrap_uniqueness.rs             → Bootstrap-uniqueness validation
  src/release_args.rs                     → Release arg validation
  src/package_release.rs                  → Release planning + pure package.json transforms
  src/apply.rs                            → TypeDB schema application
crates/ont/                               → Binary (clap + ratatui, depends on vibe-ontology)
  crates/ont/src/cli/{validate_package,validate_migration,diff,release,corpus}.rs → subcommands
bin/ontology-{release,validate-package}   → Stable cross-repo shims → build+exec `ont`
.mise/tasks/                              → Operator entrypoints (check, test, build, lint, release, …)
crates/vibe-ontology/tests/{schema_apply,bootstrap_apply}.rs → live-TypeDB integration tests (spin a fresh server via TYPEDB_BIN)
docs/                                     → Architecture docs, playbooks, contracts
```

See `docs/rust-workspace.md` for the Rust crate layout and `docs/corpus-runner.md` for the `ont corpus` command surface.

### Responsibility Boundaries

This repo **owns**:
- release automation for the ontology repos (the `ont release` flow)
- the `vibe-ontology` library that other Vibe Machine products consume
- the `ont` CLI/TUI binary

This repo **does not own**: ontology-specific schema semantics, package-local generation, translation logic, or corpus content (which lives in the ontology repos themselves).

### Release Contract

Target ontology repos must expose three npm scripts:
- `refresh:package-contract` — generate/refresh package manifest
- `validate:bootstrap` — validate manifest and package consistency
- `test:typedb-bootstrap` — validate TypeDB bootstrap

### Release Flow

1. Verify target repo is clean (no uncommitted changes)
2. Compute next version (bump or explicit `--version`)
3. Rewrite versioned manifest paths in `package.json` fields (`manifests`, `provenance.manifest`, `assembly.generatedArtifacts`, `upstream.tag`)
4. Run refresh → validate → test scripts in sequence
5. Create release commit (`Release <pkg> v<version>`) and tag (`v<version>`)
6. Push branch and tag (unless `--no-push`)

### Modes

- `--bump <patch|minor|major>` or `--version <x.y.z>` — full release
- `--dry-run` — plans release without mutating git
- `--validate-only` — runs checks in ephemeral worktree

## Tech Stack & Conventions

**Rust workspace:**
- **Rust 1.83** (pinned via mise.toml), edition 2021, `unsafe_code = "forbid"` workspace-wide
- **Library:** `vibe-ontology` (zero CLI/TUI deps; embeddable in OneApp/Lingo)
- **Binary:** `ont` depends on `vibe-ontology` + clap + ratatui + tokio
- **Testing:** `cargo test --workspace`; parity tests live beside each module. Live-TypeDB integration tests (`crates/vibe-ontology/tests/schema_apply.rs`, `bootstrap_apply.rs`) spin a fresh `typedb server` via `TYPEDB_BIN` and fail loudly if no binary is found (they must not silently skip). `mise.toml` sets `TYPEDB_BIN`, so `mise run test` runs them.
- **Lints:** `cargo clippy --workspace --all-targets -- -D warnings` must pass
- **Tags:** `v<semver>` format; release commit messages: `Release <name> v<version>`

**Release flow shells npm in the *target* repos:** `ont release` invokes the
consumer repo's `refresh:package-contract` / `validate:bootstrap` /
`test:typedb-bootstrap` npm scripts (which are still Node and belong to those
repos), so `node` remains pinned in `mise.toml` for release runs even though the
tooling itself is all-Rust.

<!-- BEGIN BEADS INTEGRATION v:1 profile:minimal hash:ca08a54f -->
## Beads Issue Tracker

This project uses **bd (beads)** for issue tracking. Run `bd prime` to see full workflow context and commands.

### Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --claim  # Claim work
bd close <id>         # Complete work
```

### Rules

- Use `bd` for ALL task tracking — do NOT use TodoWrite, TaskCreate, or markdown TODO lists
- Run `bd prime` for detailed command reference and session close protocol
- Use `bd remember` for persistent knowledge — do NOT use MEMORY.md files

## Session Completion

**When ending a work session**, you MUST complete ALL steps below. Work is NOT complete until `git push` succeeds.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **PUSH TO REMOTE** - This is MANDATORY:
   ```bash
   git pull --rebase
   bd dolt push
   git push
   git status  # MUST show "up to date with origin"
   ```
5. **Clean up** - Clear stashes, prune remote branches
6. **Verify** - All changes committed AND pushed
7. **Hand off** - Provide context for next session

**CRITICAL RULES:**
- Work is NOT complete until `git push` succeeds
- NEVER stop before pushing - that leaves work stranded locally
- NEVER say "ready to push when you are" - YOU must push
- If push fails, resolve and retry until it succeeds
<!-- END BEADS INTEGRATION -->
