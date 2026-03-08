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

Current shared command surface includes:

- `mise` runtime/tool pinning
- shared repo layout for CLI development
- task entrypoints under `.mise/tasks`
- a release command that performs shared release orchestration for ontology repos
- a validate-only mode for non-mutating refresh and validation gates

## Layout

- `bin/` user-facing command entrypoints
- `src/cli/` CLI command implementations
- `src/lib/` reusable support code
- `docs/` design and architecture notes
- `tests/` smoke and unit tests
- `.mise/tasks/` runnable project tasks

## Wrapper Contract

Ontology repos should keep thin wrappers and let `ontology-tooling` own release orchestration.

See [docs/repo-wrapper-pattern.md](docs/repo-wrapper-pattern.md).

## Status

The foundation slice is in place and `ontology-release` now owns the shared version/refresh/validation/commit/tag flow for repos that expose the standard package-contract scripts.
