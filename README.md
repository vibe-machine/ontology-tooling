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
./bin/ont validate-package --repo ../ontology-gist
./bin/ont check-questions --repo ../ontology-lingo              # run the competency questions
./bin/ont check-questions --repo ../ontology-lingo --write-generated  # regenerate the files made from the questions
```

## Current Scope

The current command surface is:

Current shared command surface includes:

- `mise` runtime/tool pinning
- `ont validate-package` for authoritative package-contract validation, including the
  competency-questions check (below)
- `ont check-questions` to run a package's competency questions against a throwaway TypeDB
- shared repo layout for CLI development
- task entrypoints under `.mise/tasks`
- a release command that performs shared release orchestration for ontology repos
- validate-only and dry-run `mise` tasks for local operator workflows

## Competency questions

Every package must declare the questions its model answers. In `package.json`, set
`"competencyQuestions": "competency/questions.json"`, or `{"waived": "<reason>"}` for a package
that deliberately has none. The questions file (`formatVersion: 1`) lists sample-data
`fixtures` and the `questions`, each with a unique `id` and `number`, a `group`, the `text`, a
`query` file holding exactly one read query, and optionally a `title` (short name of the query) and
an `explanation` (one or two plain sentences on how the query answers the question). Every `.tql`
file under `queries/` must be used by a question or listed in `package.json` `queries`.

The app shows a package's questions only when they are installed in its database as
`StoredQuestion` / `StoredQuery` records joined by `storedAnswer`, so the questions file is also
turned into two generated, installed files:

- `schema/stored-queries.tql`, the exact schema OneApp declares in `StoredQuerySchema.swift`,
  listed in `schemas` as `stored-queries`;
- `data/<package>-questions.tql`, listed in `data`: per question one `StoredQuery`
  (`<package>/<slug of the title>`, with title, query text without its leading comments, package,
  version and explanation) and one `StoredQuestion` (`<package>/q-NN`, with text, package,
  version and order), joined by `storedAnswer`. Each record is `put` on its key and its other
  attributes are set with `update`, so a re-install or an upgrade leaves one record per key with
  the latest text. `storedVersion` is the package version at which an entry last changed: an
  unchanged entry keeps the version already in the file; a new or changed one gets the current
  version.

- `ont validate-package` checks all of this statically (and so do `ont release` and
  `ont release --validate-only`). It also fails when `docs/competency-questions.md` exists but
  differs from what the questions render, when the package does not list the two installed files
  in `schemas`, `data` and `assembly.loadOrder`, or when either differs from what would be
  generated.
- `ont check-questions --repo <path>` starts a throwaway TypeDB, loads the dependencies (sibling
  `ontology-<name>` repositories), the package's schemas and data in `assembly.loadOrder`, then the
  fixtures, and runs each query. A question passes when its query runs and returns at least one
  row. On success it writes `competency/last-run.json` with a digest of the questions file, query
  files, fixtures, and the package's own schema and data files (`package.json` `schemas[].file`
  and `data`), so a schema or data change also calls for a fresh run. Set `TYPEDB_BIN` or pass `--typedb-bin` to choose the TypeDB executable.
  The run also confirms that the package installs exactly one `StoredQuestion` per question, each
  answered by a `StoredQuery`.
- `ont check-questions --repo <path> --write-generated` (alias `--write-doc`) writes
  `docs/competency-questions.md`, `schema/stored-queries.tql` and `data/<package>-questions.tql`.
  Register the two `.tql` files in `package.json` once; `ont release` regenerates all three after
  the version bump.
- `ont release` checks the questions statically before changing any file, then, after the version
  bump and refresh, runs them itself on exactly what ships, fails the release if any fails, and
  commits the new `competency/last-run.json` with the release. `ont release --validate-only`
  (and a resumed release) refuses unless the committed `competency/last-run.json` digest matches
  the committed files. Waived packages skip all of this.

## What validation refuses

Every `ont validate-package` (and so `ont release`, which validates after it refreshes) refuses:

- **Stamps that disagree with `package.json`.** The manifest file name, the manifest's
  `package.version` and `upstream.tag`, and inside every listed write file and every apply unit
  under `generated/apply-units` the `# manifest:` and `# upstream-tag:` headers, the package's own
  `moduleVersionKey` and its `manifestPath` must all name the package version. `ont
  prepare-package` re-splits listed apply units from their source file, so units never keep an
  old manifest or old contents.
- **Listed files that do not exist**: any path in `package.json` (`queries`,
  `assembly.generatedArtifacts`, migration units, besides schemas, data, manifests and the load
  order) or in the manifest (`artifacts`, `upstream.sourceArtifacts`).

`ont validate-package --released` checks a tree that claims to be publishable as the version it
declares, and additionally refuses:

- **Content changed under a released version.** When tag `v<version>` exists, the shipped content
  at `HEAD` (`package.json`, `schema/`, `data/`, `queries/`, `competency/`, `fixtures/`,
  `manifests/`, `migrations/`, `generated/apply-units/` and every path `package.json` lists) must
  equal the tag. `HEAD` at the tag, or a docs-only change, passes.
- **Hand edits after the build**: every file whose hash the manifest records must still match it.
- **A stale run record**: `competency/last-run.json` must name the package version.
- **A bump smaller than the change**, compared with the previous tag: a type, relation, attribute,
  role or function added to the schema needs at least a minor bump; one removed or renamed, or a
  `@key` added or removed, needs a major bump; anything else may be a patch.

It does not run during ordinary work, because `ont release` starts from exactly the state it
refuses (changes committed, version not yet bumped). It runs:

- in `ont release`: the bump check before any file changes (`--bump patch` on a schema addition
  fails, naming what was added), and the whole released check after the release is committed and
  tagged, before anything is pushed;
- in a pre-push hook, so a push of unbumped content is refused. Install it per ontology repository
  with `mise run install-pre-push-hook -- ../ontology-lingo`; it runs
  `../ontology-tooling/bin/ont validate-package --released --repo .`.

The bump check compares the labels declared in `define` blocks (`entity`, `relation`, `attribute`,
`struct`, `fun`, the roles in `relates` and the `owns … @key` pairs). It cannot see a change to
`sub`, `owns` or `plays` other than keys, cardinalities, value types, annotations other than
`@key`, function bodies, or changes made with `redefine`/`undefine`; those are left to the author.

Provenance `upstreamCommit` names the commit before the release commit. That is by design: the
provenance is generated before the release commit exists, and a commit cannot contain its own hash.

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

`ont validate-package` owns authoritative package-contract checks, and `ont release` builds on top of that validator for shared version/refresh/validation/commit/tag flow. `mise` remains the primary operator interface.
