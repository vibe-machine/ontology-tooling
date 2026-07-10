# ontology-tooling

Shared operational tooling for the `ontology-*` repositories in `collection-vibe-machine`.

This repo exists so release/version/tag automation and package-contract orchestration live in one dedicated place instead of being copied into individual ontology repos.

## Runtime

This repo is driven by `mise`.

Primary local flows:

```bash
mise tasks ls
mise run check
mise run test
mise run release-check -- ../ontology-trace-to-knowledge
mise run release-dry-run -- ../ontology-trace-to-knowledge patch
mise run release -- --repo ../ontology-gist --version 1.0.3
mise run release -- --help
./bin/ontology-validate-package --repo ../ontology-gist
```

## Current Scope

The current command surface is:

Current shared command surface includes:

- `mise` runtime/tool pinning
- `ontology-validate-package` for authoritative package-contract validation
- shared repo layout for CLI development
- task entrypoints under `.mise/tasks`
- a release command that performs shared release orchestration for ontology repos
- validate-only and dry-run `mise` tasks for local operator workflows

## Layout

- `bin/` stable cross-repo shims that build + exec the `ont` binary
- `crates/vibe-ontology/` durable, embeddable library (validation, release planning, apply)
- `crates/ont/` the `ont` CLI/TUI binary
- `docs/` design and architecture notes
- `tests/` live-TypeDB integration tests (pytest)
- `.mise/tasks/` runnable project tasks

## Wrapper Contract

Ontology repos should keep thin wrappers and let `ontology-tooling` own release orchestration.

See [docs/repo-wrapper-pattern.md](docs/repo-wrapper-pattern.md).

## Local Ops

The release model is local-first and `mise`-driven.

See [docs/local-release-playbook.md](docs/local-release-playbook.md).

## Status

`ontology-validate-package` owns authoritative package-contract checks, and `ontology-release` builds on top of that validator for shared version/refresh/validation/commit/tag flow. `mise` remains the primary operator interface.
