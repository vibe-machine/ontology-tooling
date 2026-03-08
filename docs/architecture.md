# Architecture

`ontology-tooling` is the shared automation home for ontology package operations.

## Responsibility Boundary

This repo should own:

- release/version/tag automation
- shared package-contract orchestration
- reusable validation orchestration
- shared CI/release workflow helpers

This repo should not own:

- ontology-specific schema semantics
- ontology-specific translation logic
- package-local documentation content

## Execution Model

- `mise.toml` pins the runtime/tooling contract
- `.mise/tasks/` provides local and CI entrypoints
- `bin/` exposes stable command names
- `src/cli/` implements commands
- `src/lib/` holds reusable internal helpers

## First Command

The first production command is `ontology-release`.

Target contract:

1. update package version state
2. rewrite versioned manifest paths
3. run package refresh
4. run bootstrap validation
5. run TypeDB bootstrap validation
6. create release commit
7. create matching git tag
8. push commit and tag

The current scaffold only establishes the command surface and argument parsing boundary.
