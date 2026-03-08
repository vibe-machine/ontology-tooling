# ontology-tooling

Shared operational tooling for the `ontology-*` repositories in `collection-vibe-machine`.

This repo exists so release/version/tag automation, package-contract orchestration, and future shared CI helpers live in one dedicated place instead of being copied into individual ontology repos.

## Runtime

This repo is driven by `mise`.

Primary local flows:

```bash
mise tasks ls
mise run check
mise run test
mise run release -- --help
```

## Current Scope

The initial production command planned for this repo is `ontology-release`.

Current foundation work includes:

- `mise` runtime/tool pinning
- shared repo layout for CLI development
- task entrypoints under `.mise/tasks`
- a release-command scaffold with argument parsing and dry-run help

## Layout

- `bin/` user-facing command entrypoints
- `src/cli/` CLI command implementations
- `src/lib/` reusable support code
- `docs/` design and architecture notes
- `tests/` smoke and unit tests
- `.mise/tasks/` runnable project tasks

## Status

This is the foundation slice for the `ontology-tooling` roadmap. The actual end-to-end release automation is tracked separately and should be implemented in the next feature slice.
