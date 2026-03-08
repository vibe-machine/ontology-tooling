# Architecture

`ontology-tooling` is the shared automation home for ontology package operations.

## Responsibility Boundary

This repo should own:

- release/version/tag automation
- shared package-contract orchestration
- reusable validation orchestration

This repo should not own:

- ontology-specific schema semantics
- ontology-specific translation logic
- package-local documentation content

## Execution Model

- `mise.toml` pins the runtime/tooling contract
- `.mise/tasks/` provides the primary local operator entrypoints
- `bin/` exposes stable command names
- `src/cli/` implements commands
- `src/lib/` holds reusable internal helpers
- releases are executed from a local workstation or agent environment, not GitHub Actions

## First Command

The first production command is `ontology-release`.

Target contract:

0. optionally run validate-only refresh/validation with no git mutation
1. update package version state
2. rewrite versioned manifest paths
3. run package refresh
4. run bootstrap validation
5. run TypeDB bootstrap validation
6. create release commit
7. create matching git tag
8. push commit and tag

The command expects a target ontology repo to expose:

- `refresh:package-contract`
- `validate:bootstrap`
- `test:typedb-bootstrap`

That keeps package-specific generation local while moving the release lifecycle into shared tooling.
