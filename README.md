# ontology-tooling

Shared operational tooling for the `ontology-*` repositories in `collection-vibe-machine`.

This repo exists so release/version/tag automation and package-contract orchestration live in one dedicated place instead of being copied into individual ontology repos.

## Runtime

This repo is driven by `mise`.

`bin/ont` is the one way to run the CLI, from here, from mise tasks, and from the ontology
repositories' npm scripts (`../ontology-tooling/bin/ont ...`). It is a tracked script, not the
binary: every call first runs `cargo build --release -p ont` from this directory and then runs
`target/release/ont`. When nothing changed, cargo's check costs about 0.2 s; when the source
changed, the call rebuilds first (progress on stderr, so `--format json` output stays clean). If
the source does not build, or cargo is missing, `bin/ont` refuses with a message naming
`mise run build-ont` instead of running an old binary. It prefers the rustup proxy
(`~/.cargo/bin/cargo`), which honours `rust-toolchain.toml`.

Primary local flows:

```bash
mise tasks ls
mise run check
mise run test
mise run qualify ../ontology-lingo              # is the candidate fit to release? PASS or FAIL
mise run qualify ../ontology-lingo --quick      # the same, upgrading only from the previous release
mise run check-upgrade ../ontology-lingo        # the upgrade test alone
mise run release-check -- ../ontology-trace-to-knowledge
mise run release-dry-run -- ../ontology-trace-to-knowledge patch
mise run release -- --repo ../ontology-gist --version 1.0.3
mise run release -- --help
./bin/ont qualify --repo ../ontology-lingo [--quick]
./bin/ont check-upgrade --repo ../ontology-lingo [--quick]
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
that deliberately has none. The questions file (`formatVersion: 1`) lists the `questions`, each
with a unique `id` and `number`, a `group`, the `text`, a `query` file holding exactly one read
query, and optionally a `title` (short name of the query), an `explanation` (one or two plain
sentences on how the query answers the question) and `expectedEmpty` (the reason, when the
question is meant to find nothing on the sample data). Every `.tql` file under `queries/` must be
used by a question or listed in `package.json` `queries`. The old `fixtures` list is refused: the
questions run on the package's sample data (below).

### Sample data

The questions run against the same records the app runs them against. The app builds a database
`sample-<package>` from the package, its dependencies and the files `package.json` lists under
`sampleData`, and shows a Run button on a package's questions only when that list is non-empty
(OneWorkspace `PackageSampleDatabase.swift`, `QuestionRunModel.swift`). It applies each listed
file as **one** write query in its own transaction, in listed order; sample data is never installed
with the package.

- Layout, as the app's Lab writes it (beads): `sandbox/sample-data/NNNN.tql`, one query per file,
  listed in order in `sampleData`.
- Or keep one source, `sandbox/sample-data.tql`, with queries separated by lines reading `end;`.
  `ont prepare-package` (and so `ont release`) writes the numbered files from it, each headed
  "Generated from sandbox/sample-data.tql", removes numbered files it no longer makes, and sets
  `sampleData`. Edit the source, never the numbered files.

`ont validate-package` refuses: a package with competency questions (not waived) and no
`sampleData`; a `sampleData` entry listed twice, missing, or also in `schemas`, `data` or
`assembly.loadOrder` (the app refuses these too); a file that is not one write query (an `end;`
line, a schema query, no write stage), holds more than one `match` (later stages run once per row
it found), or breaks the write-unit limits (50 blocks, 50,000 characters, 1000 stages); and, when
`sandbox/sample-data.tql` exists, listed files that differ from what it generates.

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
  `ontology-<name>` repositories), the package's schemas and data in `assembly.loadOrder`, then each
  `sampleData` file as one write query, and runs each query. A question passes when its query runs
  and returns at least one row; a question that returns no rows fails ("add sample records it
  should find, or mark the question expectedEmpty with the reason"); one marked `expectedEmpty`
  passes only with no rows. Each line prints the row count; read them. On success it writes
  `competency/last-run.json` with a digest of the questions file, the sample-data files, query
  files, and the package's own schema and data files (`package.json` `schemas[].file` and
  `data`), so a schema, data or sample change also calls for a fresh run. Set `TYPEDB_BIN` or pass `--typedb-bin` to choose the TypeDB executable.
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
- **Fields the app needs**: `description` and `prefix` (the app cannot read a `package.json`
  without them) and `source` (the app finds the installed version by it and cannot upgrade
  without it).
- **Sample data the app could not load**, and competency questions without sample data (see
  [Sample data](#sample-data)).
- **Listed files that do not exist**: any path in `package.json` (`queries`,
  `assembly.generatedArtifacts`, migration units, besides schemas, data, manifests and the load
  order) or in the manifest (`artifacts`, `upstream.sourceArtifacts`).

`ont validate-package --released` checks a tree that claims to be publishable as the version it
declares, and additionally refuses:

- **Content changed under a released version.** When tag `v<version>` exists, the shipped content
  at `HEAD` (`package.json`, `schema/`, `data/`, `queries/`, `competency/`, `fixtures/`,
  `sandbox/`, `manifests/`, `migrations/`, `generated/apply-units/` and every path `package.json`
  lists, `sampleData` included) must
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

## Qualifying a release

`ont qualify --repo <path> [--quick]` is the one command that says whether a candidate is fit to
release. It runs, stopping at the first failure and printing one line per stage with its time:

1. `validate-package`: the static contract, version stamps, and the upgrade rules below;
2. `check-questions`: the competency questions on a throwaway TypeDB (writes
   `competency/last-run.json`);
3. the package's own tests: `npm test` when the package has a `test` script, otherwise its
   `validate:bootstrap` and `test:typedb-bootstrap` scripts;
4. `check-upgrade`: the upgrade test below.

It ends with `PASS` or `FAIL`. On `PASS` it prints the one check it cannot run, because it needs
the app's code: the app's own install and upgrade, in OneWorkspace,
`mise run check-package-release <candidate> <dependency checkouts…>` (the exact command is
printed; it accepts `--quick`, `--install-only`, `--time-limit <seconds>` and
`--from-backup [backup folder]`, which also restores a copy of the app's newest backup of
`knowledge` (or the one given) into the throwaway server and upgrades it; the live server is never
contacted). `ont release`
runs `qualify` in full on the bumped tree before it commits and tags.
Package Node scripts keep working; new checks go into `ont`, not into each package.

## Upgrades

How the app upgrades an installed release (OneWorkspace, `OntologyViewModel+TypeDB.swift`):
installing or updating a package calls `applySchema`, which reads the installed version from the
database's provenance and asks `migrationPlan(forSchemaApplication:)` (around line 3459):

- **A `migration` plan in `package.json` whose `from` covers the installed version, and whose `to`
  is the package version:** `applyMigrationPlan` runs its phases in the order declared, each unit in
  its own transaction (an assertion must return rows; a `schema` unit is one schema transaction; a
  `write` unit is one write query). Then the package schemas are applied, the package's schema
  documentation is deleted and re-applied, and the provenance files are re-applied
  (`applyPackageBootstrapExecution`, `finalWriteUnits`).
- **Else an upgrade file** `migrations/v<installed>-to-v<target>.tql`, or a chain of them
  (`PackageBootstrapPlanner.migrationDiff`): the package schemas, then each file as **one** query,
  then documentation and provenance as above.
- **Else the upgrade is refused:** "no migration plan matches installed version"
  (`PackageMigrationPlanner.makePlan`). The app re-applies every data file only when it cannot
  read an installed version from provenance (it then installs as if fresh) or in its
  knowledge-bootstrap reconcile; an author cannot rely on either.

So a package with an earlier release needs a plan for every release it upgrades from, and a plan,
not a single file, is what `ont release` writes. In one query every stage runs once per row the
stages before it produced, so changes chained in one upgrade file multiply (a `match` on a
provenance path that every release records runs everything after it once per record).

**What a package declares.** `migration.supportsUpgradeFrom` lists the installed releases the
package upgrades from; every other release is declared not supported (the app refuses it, and the
user installs fresh). It must cover the previous release. `scripts` must have
`"test:typedb-migration": "../ontology-tooling/bin/ont check-upgrade --repo ."` run from `"test"`.
A first release is exempt. `ont release` adds a `migration` section covering the previous minor
when there is none; widen `supportsUpgradeFrom` (for example `">=1.0.1 <1.2.0"`) to support more.

**What `ont release` generates.** For every released tag `supportsUpgradeFrom` covers, one plan
(`"generated": true`, rewritten on every release; hand-written plans are kept):

1. `preflight`: the source version is recorded;
2. `schema`: `migrations/v<from>-to-v<to>-schema.tql`, the package schema files without their
   functions. Defining a type that is installed changes nothing, so it adds exactly what the
   release adds before the writes that use it, and is safe to repeat after an interrupted upgrade.
   Functions are left to the package schema step after the plan, which redefines them;
3. `migrate`: the write units `migrations/v<from>-to-v<to>/NNNN.tql`, then the provenance files.
   Only rows that changed since that release are written, matched by key across all of its data
   files; schema documentation and provenance are left out of the units (the app rebuilds them).
   Put/update groups from one data file share a unit within the data write-unit limits; every
   `match` change is a unit of its own and selects its record by a `@key` or `@unique`
   attribute; a changed row whose type has no such key is refused with the reason;
4. `verify`: the new version is recorded.

**What `validate-package` refuses**, each with a message that says what to do: a missing plan or
`supportsUpgradeFrom` for the previous release; a release `supportsUpgradeFrom` covers that no
plan upgrades from; a missing upgrade test script; a single-file upgrade to the package's own
version; and in any `migrations/v<from>-to-v<to>/` unit, more than one `match`, a `put` after a
`match`, a `match` not selected by a key of a type the schemas define, a variable declared with
two types (TypeDB's INF11, as in lingo's generated `v1.0.0-to-v1.0.1.tql`), or a unit over the
write-unit limits or 1000 stages. Single files to earlier versions are released history and are
not re-checked.

**`ont check-upgrade --repo <path> [--quick]`** upgrades the candidate the way the app does, on one
throwaway TypeDB with a fresh database per case, and prints one line per case and a total:

- **from each released tag on its own:** the dependencies (sibling `ontology-<name>` working
  trees), that tag, then the app's path for that installed version. A tag outside
  `supportsUpgradeFrom` that the app refuses is reported "not supported, as declared" (without a
  database when it has neither plan nor upgrade file); a tag inside it must upgrade;
- **from the previous release after an interrupted attempt:** the candidate's schemas already
  applied before the upgrade, as on a database whose upgrade stopped part-way;
- **through every release:** the oldest tag that installs, each later tag upgraded in order as
  the app would at the time (a failed or refused historical step is noted and that release is
  installed over the database instead), then the candidate. Several releases' records then sit
  side by side, which is when a `match` not selected by a key multiplies.

After each, the candidate's module version must be recorded and, for a package that installs its
competency questions, every question must be a `StoredQuestion` with its answer. Each unit has a
time limit (`--unit-timeout`, 120 s). A tag that cannot be installed on today's TypeDB and
dependencies is reported by name as "could not be used as a base". Only a failed upgrade from an
installable, supported base fails the run. `--quick` runs only the previous release, the
interrupted attempt and the lived-through database. The full run for ontology-lingo's eleven tags
takes about 70 s.

## Layout

- `bin/ont` the entrypoint script: rebuilds the release binary when its source changed, then runs it
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
