# CI Workflow Contract

The shared ontology CI workflow is designed as a reusable GitHub workflow hosted in `ontology-tooling`.

## Inputs

- `target-path`: checkout path for the caller repository
- `mode`: `validate-only`, `dry-run`, or `release`
- `bump`: `patch`, `minor`, or `major`
- `explicit-version`: optional exact version for release mode
- `push`: whether release mode should push branch and tag
- `tooling-ref`: optional git ref for the `ontology-tooling` checkout

## Behavior

- `validate-only` runs refresh and both validators with no commit, tag, or push
- `dry-run` computes the release plan without mutating the target repo
- `release` performs version rewrite, refresh, validation, commit, tag, and optional push

## Runtime

- the workflow uses `jdx/mise-action@v3` per the official `mise` GitHub Actions guidance
- commands execute from the checked-out `ontology-tooling` repo via `mise run`

## Proving Path

The shared command was dry-run exercised against `ontology-trace-to-knowledge`:

- target repo: `ontology-trace-to-knowledge`
- mode: `dry-run`
- result: planned bump `1.0.1 -> 1.0.2`

Current gap:

- repos that do not expose `refresh:package-contract` are not yet ready for the shared release path and must add that thin wrapper first
