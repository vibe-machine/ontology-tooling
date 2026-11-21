---
name: ontology-extend
description: The process for changing and releasing an ontology package (a TypeQL package built on gist, in an `ontology-*` repository) so it installs and upgrades cleanly in the app. Use when (a) adding or changing an entity, relation, attribute or category in any ontology package; (b) writing or changing competency questions, their queries or sample data (`sampleData`, `sandbox/sample-data.tql`); (c) bumping a package version, cutting a release, or tagging and pushing one; (d) writing or debugging a migration plan or an upgrade from an installed version; (e) running `ont` commands (`check-questions`, `validate-package`, `qualify`, `check-upgrade`, `release`) or `mise run check-package-release`; (f) a package install or upgrade fails in the app. Complements `typeql` (syntax), `modeling` (schema design), `ontology-repo-layout` (where repositories live) and `typedb` (the app's bootstrap pipeline).
---

# Changing and releasing an ontology package

A package is a set of TypeQL schema and data files plus a `package.json` contract. The app
installs it from a git tag into a shared database beside gist and every other package, and later
upgrades that database to newer tags. Every rule below exists because a release broke without it.

**The app is the contract.** Before adding anything to a package (a field, a file, a
convention), find what the app reads and how; what it does not read never reaches the user. The
tooling copies the app's rules and can lag. Table of app expectations against tooling checks, and
the gaps only the app's check catches:
[references/app-package-contract.md](references/app-package-contract.md).

Follow the steps in order. Each says what to run, what passing looks like, and why.

## 1. Before you start

- Keep sibling checkouts beside the package: `ontology-gist`, `ontology-tooling` (`bin/ont`), every
  other `ontology-*` package installed beside this one, and OneWorkspace for the app check. Package
  scripts reach them by relative paths (`../ontology-tooling/bin/ont`). See `ontology-repo-layout`.
- `bin/ont` rebuilds the `ont` binary through cargo when its source changed, and refuses (naming
  `mise run build-ont`) if it cannot; never run `target/release/ont` directly.
- `package.json` must carry `description`, `prefix` and `source`: the app cannot read a package
  without the first two, and finds the installed version by `source`.
- Read the package's `AGENTS.md` (or `CLAUDE.md`), `README.md` and `package.json`. Note the version and last tag (`git tag --sort=-v:refname | head -3`); shipped files that differ from it
  are unreleased and need a new version, never the tagged one (step 8).
- Make sure the repository has the pre-push hook that refuses unreleased content:
  `mise run install-pre-push-hook -- ../ontology-<name>` (run in `ontology-tooling`). It runs
  `../ontology-tooling/bin/ont validate-package --released --repo .` before every push.
- Do not commit, push or tag without the owner's word. A release is a push: the app reads tags
  from GitHub, so a pushed tag reaches every user.

## 2. Questions first

Write the questions the change must answer before touching the schema.

- Source: `competency/questions.json`, with `"competencyQuestions": "competency/questions.json"`
  in `package.json` (or `{"waived": "<reason>"}` for a package that deliberately has none).
  Format and generated files: [references/competency-questions.md](references/competency-questions.md).
- One read query per question in `queries/`, run on the package's `sampleData` as the app runs it
  (database `sample-<package>`; no `sampleData`, no Run button). Write invented records in
  `sandbox/sample-data.tql`, queries separated by `end;` lines; `ont prepare-package` / `ont
  release` split it into numbered files; never split by hand. `fixtures` is refused. A question
  with no rows fails unless marked `expectedEmpty` with the reason.
- A question the model cannot answer is a finding: change the model. Never weaken the question to
  fit the model; that hides the gap the questions exist to find.

## 3. Design on gist, the way existing packages do

- Before inventing a format, file or convention, find how the app reads it and how an existing
  package (gist, beads, Lingo) ships it, and copy that. Lingo 1.0.2 published its questions in a
  made-up format the app never read; 1.1.1 ran them on fixtures the app never loads, so it had no
  Run button; 1.1.0 wrote its upgrade as one file instead of gist's plans of small units (step 6).
- Build every entity on a gist type and every relation on a gist relation where one fits; name the
  roles in plain words. Use the current gist role names, which changed in gist 5:
  [references/gist-types.md](references/gist-types.md),
  [references/gist-relations.md](references/gist-relations.md),
  [references/schema-patterns.md](references/schema-patterns.md).
- Fixed lists are category types with seed values, never `@values` strings.
- Check every new type, relation, role and attribute name against every package installed beside
  this one, because they all share one database: `rg -nw '<Name>' ../ontology-*/schema/*.tql`.
- Do not change a type a dependency owns (no added `owns`, parent, annotation or role in someone
  else's relation); the app refuses the install and the tooling does not check it. A dependency's
  type may play a role in a relation this package declares.
- Give every record type a `@key`. An upgrade can change a record only by matching its key;
  records without one pile up, one set per installed version (step 6).

## 4. Write the schema, the documentation and the data

- Schema in `schema/<package>.tql` (or a new file listed in `schemas` and `assembly.loadOrder`
  after what it depends on).
- Every new type needs its description in the package's schema-docs generator (for example
  `tools/generate_<package>_schema_docs.mjs`); an undescribed type gets a generic sentence.
- Data files must be safe to run twice (a reinstall, or an upgrade run again after it stopped
  part-way): for a keyed entity, `put` on the key, then `update` the other attributes. Traps:
  [references/typedb-limits-and-traps.md](references/typedb-limits-and-traps.md).
- Keep `displayName` (and so the schema-docs module name) unchanged. It is recorded in every
  installed database; renaming it made 1.0.0's data clash with the existing module record
  (`[CNT9] @unique violated … SchemaModule … moduleKey`).
- Regenerate everything generated: `npm run refresh:package-contract`. Never hand-edit generated
  files (the package's `AGENTS.md` lists them). Oversized data files are split into
  `generated/apply-units/`; `ont prepare-package` and `ont release` re-split them every time.

## 5. Questions, queries, sample data

```bash
../ontology-tooling/bin/ont check-questions --repo . --write-generated  # docs + installed question files
../ontology-tooling/bin/ont check-questions --repo .                    # run every query
```

Passing: every question reports `passed` (rows > 0, or 0 when `expectedEmpty`), and
`competency/last-run.json` is rewritten. Sample files must each be one write query with at most one
`match`, within 1000 stages, 50 blocks and 50,000 characters ([references/competency-questions.md](references/competency-questions.md#sample-data)). `--write-generated` turns the questions into `docs/competency-questions.md`,
`schema/stored-queries.tql` and `data/<package>-questions.tql` (one `StoredQuestion` and one
`StoredQuery` per question). Register both `.tql` files once in `schemas`, `data` and
`assembly.loadOrder`. Worked example: Lingo 1.1.2 (27 questions, two sample files).

Read the numbers in the output, not only the pass line. A count you did not expect (a "Total
answers" far larger than the data) is a finding; one was ignored before Lingo 1.1.0 failed.

## 6. Declare the upgrade: a migration plan of small units

A package with an earlier release must say how the app upgrades each installed release. How the
app chooses, exactly: [references/app-install-and-upgrade.md](references/app-install-and-upgrade.md).
In short: a matching `migration` plan runs its phases in order, each unit in its own transaction;
else an exact upgrade file `migrations/v<from>-to-v<to>.tql` runs as **one** query in one
transaction; else the app refuses ("no migration plan matches installed version"). A folder of
units is ignored unless a plan lists them.

- **Never ship a single-file upgrade.** Lingo 1.1.0 did. Its file chained many `match` stages not
  selected by a key; on a database that had lived through several versions the rows multiplied
  about 221,000-fold and TypeDB closed the transaction (`[CXN07] The transaction is closed`).
- In `package.json`, `migration.supportsUpgradeFrom` lists the releases this one upgrades from
  (it must cover the previous release); every other release is thereby declared not supported,
  and its users install fresh. Lingo 1.1.1: `[">=1.0.1 <1.2.0"]`.
- `ont release` writes one generated plan (`"generated": true`) per earlier tag that
  `supportsUpgradeFrom` covers, phases `preflight`, `schema`, `migrate`, `verify`; hand-written
  plans are kept. Widen or narrow the ranges; do not hand-write what it generates.
- `scripts` needs `"test:typedb-migration": "../ontology-tooling/bin/ont check-upgrade --repo ."`,
  run from `"test"`. A first release is exempt.
- Each unit is one transaction and must: hold at most one `match`; select every matched record by
  a `@key` or `@unique` attribute; have no `put` after its `match`; not give one variable two
  types; stay within 50 executable blocks, 50,000 characters and 1000 stages.
  `ont validate-package` refuses each of these with a message saying what to do.

## 7. Qualify

```bash
npm run refresh:package-contract
../ontology-tooling/bin/ont qualify --repo .          # the whole gate
../ontology-tooling/bin/ont qualify --repo . --quick  # while iterating
```

`ont qualify` runs, stopping at the first failure: `validate-package`, `check-questions`, the
package's own tests (`npm test`), then `check-upgrade`. Passing: one line per stage, then
`qualify: PASS`. `check-upgrade` upgrades a throwaway database from every earlier tag, from the
previous release after an interrupted attempt, and on a database that lived through every
release (where unkeyed matches multiply). `--quick` upgrades only from the previous release plus
those last two. Cases: [references/app-install-and-upgrade.md](references/app-install-and-upgrade.md#testing-upgrades).

## 8. Choose the version

Every change to shipped content gets its own version bump, made by `ont release`. Never ship
content under a version that is already tagged: the app skips a package already at that version,
so the new content never arrives.

| Change | Bump |
|---|---|
| Type, relation, role, attribute or function added | minor |
| Anything removed or renamed, a key changed, a type re-parented | major (and a hand-written plan) |
| Data, documentation, queries or questions only | patch |

`ont release` compares the labels declared in `define` blocks with the previous tag and refuses a
bump that is too small. It cannot see changes to `sub`, non-key `owns`/`plays`, cardinalities,
annotations, function bodies or `redefine`/`undefine`; judging those is your job.

## 9. Release locally

Commit the source change first (with the owner's word): `ont release` refuses a dirty tree.

```bash
npm run release -- --bump <patch|minor|major> --dry-run   # preview
npm run release -- --bump <patch|minor|major> --no-push   # commit + tag locally only
```

`ont release` bumps the version, regenerates the question files, the migration plans and units,
runs `refresh:package-contract`, then `ont qualify` in full on exactly what ships, commits
`Release <name> v<version>`, tags `v<version>`, and runs `ont validate-package --released`.

- Never edit, amend or delete a file after it. Deleting a file by hand after a release left the
  manifest naming a file that did not exist. If its output is wrong, fix the tool in
  `ontology-tooling` and release again.
- Always pass `--no-push`; the push comes after step 10.

## 10. The app's own check, before pushing

`ont qualify` drives TypeDB directly; it is not the app's code. Then, from OneWorkspace:

```bash
mise run check-package-release <this checkout> <gist checkout> [other dependency checkouts]
```

It runs the committed HEAD through the app's Install / Upgrade code on throwaway TypeDB servers:
a fresh install, an upgrade from each earlier tag, and a lived-through database. Passing: final
line `PASS: <name> <version> installs and upgrades through the app (full).` Options: `--quick`,
`--install-only`, `--time-limit <seconds>` (per step, default 120), `--from-backup [folder]` (also
upgrades a restored copy of the app's newest backup). It is the only check for the gaps in
[references/app-package-contract.md](references/app-package-contract.md). It re-signs a running
sandbox app: run `mise run dev --sandbox` afterwards.

If anything fails, the release is still local: with the owner's word, delete the tag and the
release commit (`git tag -d v<next> && git reset --hard HEAD~1`), fix the cause, and release
again. Once pushed, a release is never changed; fix forward with a new version.

## 11. Push, with the owner's word

```bash
git push --atomic origin main v<version>
```

Branch and tag go together so the app never sees a tag without its commit.

## 12. After release: verify in the app

Do not say what is installed from memory or from a passing test; read the real thing. Update the
package in the app, install it into the owner's database, then read the live database (see
`vibemachine-ontology`):

```typeql
match $m isa OntologyModule, has moduleRepoUrl "https://github.com/vibe-machine/ontology-<name>";
  $v isa OntologyModuleVersion, has moduleVersion $ver; ontologyModuleVersionOf (version: $v, module: $m);
select $ver;
match $q isa StoredQuestion, has storedPackage "<name>"; reduce $n = count;
```

Open the package in the app: its questions appear, and Run answers them on the sample database.

## 13. When something fails

- Read the error text. TypeDB codes map to traps in
  [references/typedb-limits-and-traps.md](references/typedb-limits-and-traps.md)
  (`CXN07` row multiplication, `QEX18` stage limit, `CNT9` uniqueness, `INF11`, `FUN5`).
- A question returns no rows: the model or the sample data is wrong, not the question (step 2).
- `ont` refuses: it is guarding a rule above. Fix the cause or the tool; never hand-edit generated
  or released files.
- An upgrade failed in a real database: stop. The database may be half upgraded. Report the log,
  restore from the app's backup with the owner, and fix forward with a new version.
- File a bead for anything found that is not fixed now.

## Checklist

- [ ] Looked at how gist, beads or Lingo already do it before inventing anything
- [ ] Found what the app reads before adding anything; `description`, `prefix`, `source` set
- [ ] Questions written first; each query returns rows on `sampleData` (or is `expectedEmpty`)
- [ ] Every type on gist and keyed; names checked against sibling packages; no change to a
      dependency's type
- [ ] New types described; `displayName` unchanged; data files safe to run twice
- [ ] `supportsUpgradeFrom` declared; `test:typedb-migration` in `test`; no single-file upgrade
- [ ] `ont qualify` PASS, and its counts read
- [ ] Bump matches the change; released with `ont release --no-push`; nothing edited after it
- [ ] `mise run check-package-release` PASS
- [ ] Pushed `main` and tag together, only with the owner's word
- [ ] Version and questions confirmed in the live database
