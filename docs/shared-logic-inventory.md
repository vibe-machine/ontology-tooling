# Shared Logic Inventory

This inventory captures the reusable package-contract mechanics currently duplicated across the ontology repos.

## Repos Reviewed

- `ontology-gist`
- `ontology-template`
- `ontology-trace-to-knowledge`
- `ontology-beads`
- `ontology-gastown`
- `ontology-vibemachine`

## Stable Shared Surface

These mechanics are cross-repo and belong in `ontology-tooling`:

- version planning from `package.json.version`
- versioned manifest path rewrites in `package.json`
- release commit and git tag naming
- package refresh orchestration
- bootstrap validation orchestration
- TypeDB bootstrap validation orchestration
- git cleanliness checks before release

These mechanics stay repo-local:

- schema generation
- provenance/data generation internals
- ontology-specific refresh logic
- ontology-specific validation details

## Script Contract

The reusable script boundary for ontology repos is:

- `refresh:package-contract`
- `validate:bootstrap`
- `test:typedb-bootstrap`

The first extraction target is now complete in `ontology-release`, which uses this stable script contract instead of embedding ontology-specific generation logic.

