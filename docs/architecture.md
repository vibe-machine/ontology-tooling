# Architecture

`ontology-tooling` is the shared automation home for ontology package
operations and the durable home of the Vibe Machine `ont` CLI/TUI.

## One Surface

The repo is a single Rust workspace (`Cargo.toml`, `crates/vibe-ontology/`,
`crates/ont/`). `vibe-ontology` is the durable library other Vibe Machine
products embed; `ont` is the unified CLI/TUI over it, which owns release
orchestration for the ontology repos (`ont release`). The former Node release
tooling has been fully consolidated into this workspace; `bin/ontology-release`
and `bin/ontology-validate-package` remain as stable cross-repo shims that build
and exec `ont`. See `docs/rust-workspace.md`, `docs/corpus-runner.md`,
`docs/repo-wrapper-pattern.md`, and `docs/local-release-playbook.md`.

The two surfaces are independent. The Node tooling does not depend on the
Rust workspace and vice versa.

## Responsibility Boundary

This repo should own:

- release/version/tag automation (Node)
- shared package-contract orchestration (Node)
- reusable validation orchestration (Node)
- the `vibe-ontology` library (Rust) — corpus model, discovery, validation,
  prompt-export
- the `ont` CLI/TUI binary (Rust) — operator surface for the above

This repo should not own:

- ontology-specific schema semantics
- ontology-specific translation logic
- package-local documentation content
- corpus content (lives in the ontology repos themselves)

## Execution Model

- `mise.toml` pins the runtime/tooling contract
- `.mise/tasks/` provides the primary local operator entrypoints
- `bin/` exposes stable command names (shims → `ont`)
- `crates/ont/src/cli/` implements the `ont` subcommands
- `crates/vibe-ontology/` holds the reusable library logic
- releases are executed from a local workstation or agent environment, not GitHub Actions

## Command Boundary

The foundational production commands are:

- `ontology-validate-package` for authoritative package-contract validation
- `ontology-release` for release orchestration on top of that validation

Target contract:

0. optionally run validate-only refresh/validation with no git mutation
1. update package version state
2. rewrite versioned manifest paths
3. run package refresh
4. run package-contract validation in `ontology-tooling`
5. run bootstrap validation
6. run TypeDB bootstrap validation
7. run TypeDB migration validation when migrations are declared
8. create release commit
9. create matching git tag
10. push commit and tag

The command expects a target ontology repo to expose:

- `refresh:package-contract`
- `validate:bootstrap`
- `test:typedb-bootstrap`

That keeps package-specific generation local while moving the release lifecycle into shared tooling.
