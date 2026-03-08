# Local Release Playbook

`ontology-tooling` is a local-first operational repo.

Releases are driven with `mise`, from a developer workstation or agent environment that has:

- git push access to the ontology repositories
- `mise` installed
- Node provisioned through `mise`
- TypeDB available for the package bootstrap validator

## Core Flows

Validate a target repo without committing, tagging, or pushing:

```bash
mise run release-check -- ../ontology-trace-to-knowledge
```

This runs in an ephemeral worktree so the real checkout stays clean.

Preview the next release plan:

```bash
mise run release-dry-run -- ../ontology-trace-to-knowledge patch
```

Cut a real release:

```bash
mise run release -- --repo ../ontology-trace-to-knowledge --bump patch
```

Use an explicit version instead of a bump:

```bash
mise run release -- --repo ../ontology-trace-to-knowledge --version 1.2.0
```

## Release Contract

The target repo must expose these scripts in `package.json`:

- `refresh:package-contract`
- `validate:bootstrap`
- `test:typedb-bootstrap`

`ontology-release` will:

1. verify the target repo is clean
2. compute the next version
3. rewrite versioned manifest references
4. run `refresh:package-contract`
5. run `validate:bootstrap`
6. run `test:typedb-bootstrap`
7. create the release commit
8. create the matching git tag
9. push the branch and tag unless `--no-push` is used

## Current Position

This is the authoritative release path.

GitHub Actions is not part of the release architecture for ontology packages.
