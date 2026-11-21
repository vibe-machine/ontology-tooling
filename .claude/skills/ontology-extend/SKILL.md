---
name: ontology-extend
description: The process for changing and releasing an ontology package (a TypeQL package built on gist, in an `ontology-*` repository) so it installs and upgrades cleanly in the app. Use when (a) adding or changing an entity, relation, attribute or category in any ontology package; (b) writing or changing competency questions, their queries or sample fixtures; (c) bumping a package version, cutting a release, or tagging and pushing one; (d) writing or debugging a migration or an upgrade from an installed version; (e) running `ont` commands (`check-questions`, `validate-package`, `prepare-package`, `release`) or `mise run check-package-release`; (f) a package install or upgrade fails in the app. Complements `typeql` (syntax), `modeling` (schema design), `ontology-repo-layout` (where repositories live) and `typedb` (the app's bootstrap pipeline).
---

# Changing and releasing an ontology package

A package is a set of TypeQL schema and data files plus a `package.json` contract. The app
installs it from a git tag into a shared database beside gist and every other package, and later
upgrades that database to newer tags. Every rule below exists because a release broke without it.

Follow the steps in order. Each says what to run, what passing looks like, and why.

## 1. Before you start

- Work in the package's checkout and keep its sibling checkouts beside it: `ontology-gist` (the
  dependency, and the scripts `refresh:package-contract` calls), `ontology-tooling` (`bin/ont`),
  every other `ontology-*` package that is installed beside this one, and the app's workspace
  (OneWorkspace) for the install check. Package scripts reach them by relative paths such as
  `../ontology-tooling/bin/ont`, so the layout matters. See `ontology-repo-layout`.
- Read the package's `AGENTS.md` (or `CLAUDE.md`), `README.md` and `package.json`.
- Note the current version and the last tag: `git tag --sort=-v:refname | head -3`. If the
  working tree differs from the last tag in shipped files, that content is unreleased; it needs a
  new version, never the tagged one (rule in step 7).
- Make sure the repository has the pre-push hook that refuses unreleased content. It is installed
  per repository, from `ontology-tooling`: `mise run install-pre-push-hook -- ../ontology-<name>`.
  It adds a marked section to the repo's pre-push hook (beside the beads section, if any) that runs
  `../ontology-tooling/bin/ont validate-package --released --repo .`.
- Do not commit, push or tag without the owner's word. A release is a push: the app reads tags
  from GitHub, so a pushed tag reaches every user.

## 2. Questions first

Write the questions the change must answer before touching the schema.

- Source: `competency/questions.json`, with `"competencyQuestions": "competency/questions.json"`
  in `package.json` (or `{"waived": "<reason>"}` for a package that deliberately has none).
  Format and generated files: [references/competency-questions.md](references/competency-questions.md).
- Every question gets exactly one read query in `queries/`. Every query must return rows against
  gist + the package + small invented sample data in `fixtures/`, which is never installed.
- A question the model cannot answer is a finding: change the model. Never weaken the question to
  fit the model; that hides the gap the questions exist to find.

## 3. Design on gist

- Build every entity on a gist type and every relation on a gist relation where one fits; name the
  roles in plain words. Use the current gist role names, which changed in gist 5:
  [references/gist-types.md](references/gist-types.md),
  [references/gist-relations.md](references/gist-relations.md),
  [references/schema-patterns.md](references/schema-patterns.md).
- Fixed lists are category types with seed values, never `@values` strings: a category can be
  queried and documented; a string list cannot.
- Check every new type, relation, role and attribute name against every package installed beside
  this one, because they all share one database:
  `rg -nw '<Name>' ../ontology-*/schema/*.tql`. (Lingo's relation `reference` clashed with a
  beads attribute and had to become `questionReference`.)
- Do not change a type a dependency owns. The app refuses an install that adds `owns`, a parent,
  an annotation, or a role in someone else's relation to a dependency's type. The one allowance: a
  dependency's type may play a role in a relation this package declares.
- Before inventing a format or a convention, look at how an existing package already ships the
  same thing (beads already shipped stored questions; Lingo 1.0.2 invented repository-only
  question files and the app showed none).

## 4. Write the schema, the documentation and the data

- Schema in `schema/<package>.tql` (or a new file listed in `schemas` and `assembly.loadOrder`
  after what it depends on).
- Every new type needs its description in the package's schema-docs generator (for example
  `tools/generate_<package>_schema_docs.mjs`); an undescribed type gets a generic sentence in the
  app's documentation.
- Data files must be safe to run twice: the app reapplies them on an upgrade that has no migration
  file. For a keyed entity, `put` on the key, then `update` the other attributes. Traps:
  [references/typedb-limits-and-traps.md](references/typedb-limits-and-traps.md).
- Keep `displayName` (and so the schema-docs module name) unchanged. It is recorded in every
  installed database; renaming it made 1.0.0's data clash with the existing module record
  (`[CNT9] @unique violated … SchemaModule … moduleKey`).
- Regenerate everything generated: `npm run refresh:package-contract`. It rewrites the schema
  docs, provenance, manifest and the shared schema files. Never hand-edit generated files (the
  package's `AGENTS.md` lists them). Oversized data files are split into
  `generated/apply-units/`; `ont prepare-package` re-splits those units from their source file
  every time, and `ont release` regenerates them after the bump, so never edit a unit by hand.

## 5. Questions, queries, fixtures

```bash
../ontology-tooling/bin/ont check-questions --repo . --write-generated  # docs + installed question files
../ontology-tooling/bin/ont check-questions --repo .                    # run every query
```

Passing: every question reports `passed` with rows > 0, and `competency/last-run.json` is
rewritten. The run loads the sibling dependency checkouts, the package in `assembly.loadOrder`,
then the fixtures, into a throwaway TypeDB (`TYPEDB_BIN` or `--typedb-bin` picks the binary).

The questions reach the app only as installed records. `--write-generated` turns
`competency/questions.json` (each question with a `title` and an `explanation`) into
`docs/competency-questions.md`, `schema/stored-queries.tql` (the schema the app declares) and
`data/<package>-questions.tql` (one `StoredQuestion` and one `StoredQuery` per question, joined by
`storedAnswer`). Register both `.tql` files once in `schemas`, `data` and `assembly.loadOrder`;
`ont release` regenerates them at each version, and `check-questions` confirms the package
installs exactly one answered `StoredQuestion` per question. Worked example: Lingo 1.1.0
(27 questions).

## 6. Validate

```bash
npm run refresh:package-contract
../ontology-tooling/bin/ont validate-package --repo .
npm test
```

Passing: `validate package: <name> v<version> [ok]` and `npm test` exits 0 (it loads gist and the
package into a temporary TypeDB). `validate-package` also checks the questions statically, the
upgrade files' size limits and that every listed file exists.

## 7. Choose the version

Every change to shipped content gets its own version bump, made by `ont release` in the release
commit. Never commit or push shipped content under a version that is already tagged: the app
reads the installed version from the database and skips a package already at that version, so
the new content never arrives.

| Change | Bump |
|---|---|
| Type, relation, role, attribute or function added | minor |
| Anything removed or renamed, a key changed, a type re-parented | major (and a migration plan) |
| Data, documentation, queries or questions only | patch |

Lingo 1.0.2 added a relation as a patch. `ont release` compares the labels declared in `define`
blocks (types, roles, functions, `@key` pairs) with the previous tag before changing any file, and
refuses a bump that is too small, naming what was added. It cannot see changes to `sub`, non-key
`owns`/`plays`, cardinalities, annotations, function bodies or `redefine`/`undefine`; judging
those is your job.

## 8. Release locally

Commit the source change first (with the owner's word): `ont release` refuses a dirty tree.

```bash
npm run release -- --bump <patch|minor|major> --dry-run   # preview
npm run release -- --bump <patch|minor|major> --no-push   # commit + tag locally only
```

`ont release` bumps the version, renames the manifest, regenerates the question files, runs
`refresh:package-contract`, `validate:bootstrap`, `test:typedb-bootstrap` and the competency
questions on exactly what ships, writes the upgrade file `migrations/v<prev>-to-v<next>.tql`
when data changed, commits `Release <name> v<version>`, tags `v<version>`, and then runs
`ont validate-package --released` on the result: no shipped content changed under the tag, every
manifest hash matches its file (no hand edits), `competency/last-run.json` names this version,
and the bump is large enough.

- Release only with `ont release`. Never edit, amend or delete files after it: amending the release
  commit to delete a file left the manifest naming a file that did not exist. If its output is
  wrong, fix the tool in `ontology-tooling` and release again.
- Always pass `--no-push`; the push comes after step 9.

## 9. Test three installs before pushing

All three must pass on the local release commit. A passing fresh install says nothing about an
upgrade: Lingo 1.0.0 passed fresh, then failed half way through upgrading the owner's database
(`[QEX18] Query pipeline has 1203 stages, which exceeds the maximum allowed 1000`) and left it
half upgraded.

1. **The package's own tests**: `npm test` (already run by `ont release`). Passing: exit 0.
2. **The app's fresh install** on a throwaway database, from OneWorkspace:
   `mise run check-package-release <this checkout> <gist checkout> [other dependency checkouts]`.
   It installs each checkout's committed HEAD through the app's install path. Passing: final line
   `PASS: <name> <version> installs through the app and reads as installed.` It rebuilds and
   re-signs the app, so run `mise run dev` afterwards before anyone uses the app.
3. **An upgrade from the previous release** on a throwaway TypeDB. No command does this yet, so
   run it by hand: load gist, the previous tag (from a `git worktree`), the new schema files, then
   `migrations/v<prev>-to-v<next>.tql` as one write transaction, and read back the module
   versions and a count that proves the new content. This drives TypeDB directly; it is not the
   app's install path. Steps and the one trap (`[FUN5]` on unchanged functions):
   [references/app-install-and-upgrade.md](references/app-install-and-upgrade.md#testing-an-upgrade-by-hand).

If any fails, the release is still local: with the owner's word, delete the tag and the release
commit (`git tag -d v<next> && git reset --hard HEAD~1`), fix the cause, and release again. Once
pushed, a release is never changed; fix forward with a new version.

## 10. Push, with the owner's word

```bash
git push --atomic origin main v<version>
```

Branch and tag go together so the app never sees a tag without its commit. The pre-push hook
runs `ont validate-package --released` and refuses a push of content changed under a released
version.

## 11. After release: verify in the app

Do not say what is installed from memory or from a passing test; read the real thing.

- In the app, update the package and install it into the owner's database (the app picks the
  newest semver tag from GitHub).
- Read the live database (see `vibemachine-ontology` for the console) and confirm the version and
  the questions:

```typeql
match $m isa OntologyModule, has moduleRepoUrl "https://github.com/vibe-machine/ontology-<name>";
  $v isa OntologyModuleVersion, has moduleVersion $ver;
  ontologyModuleVersionOf (version: $v, module: $m);
select $ver;
```

```typeql
match $q isa StoredQuestion, has storedPackage "<name>"; reduce $n = count;
```

- Open the package in the app and check its questions appear.

## 12. When something fails

- Read the error text. TypeDB codes map to traps in
  [references/typedb-limits-and-traps.md](references/typedb-limits-and-traps.md)
  (`QEX18` stage limit, `CNT9` uniqueness).
- A question returns no rows: the model or the fixture is wrong, not the question (step 2).
- `ont` refuses: it is guarding a rule above. Fix the cause or the tool; never work around it by
  hand-editing generated or released files.
- An upgrade failed in a real database: stop. Do not retry blindly; the database may be half
  upgraded. Report the log, restore from the app's backup or snapshot with the owner, and fix
  forward with a new version.
- File a bead for anything found that is not fixed now.

## Checklist

- [ ] Questions written first; every question has one query that returns rows on fixtures
- [ ] Every type on gist, current gist role names, new names checked against sibling packages
- [ ] No change to a dependency's type beyond playing a role in this package's relation
- [ ] Every new type described in the docs generator; `displayName` unchanged
- [ ] Data files safe to run twice (`put` on the key, then `update`)
- [ ] `refresh:package-contract`, `ont check-questions`, `ont validate-package`, `npm test` pass
- [ ] Question files installed (`stored-queries.tql`, `<package>-questions.tql`), as beads does
- [ ] Bump matches the change; no shipped content under an existing tag
- [ ] Released with `ont release --no-push`; nothing edited after it
- [ ] Fresh install (`check-package-release`) and upgrade from the previous tag both pass
- [ ] Pre-push hook installed; pushed `main` and tag together, only with the owner's word
- [ ] Version and questions confirmed in the live database
