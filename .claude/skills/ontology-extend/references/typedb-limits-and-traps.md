# TypeDB limits and traps that have broken releases

Each entry: the rule, the symptom, the fix.

## Limits

**A query pipeline holds at most 1000 stages.** Every top-level `match`, `insert`, `put`,
`delete`, `update`, `fetch`, `reduce`, `sort`, `offset`, `limit`, `select`, `require` and
`distinct` is a stage. Symptom: `[QEX18] Query pipeline has N stages, which exceeds the maximum
allowed 1000`. It bit a single-file upgrade (`migrations/v<from>-to-v<to>.tql`), which the app
sends as one query. `ont validate-package` now rejects such a file; `ont release` splits an
oversized upgrade into units (see `app-install-and-upgrade.md` for what the app does with them).

**A write unit holds at most 50 executable blocks and 50,000 characters.** Symptom from the
tooling: `write unit '<path>' contains 51 executable blocks; max is 50`. `ont prepare-package
--repo .` splits an oversized data file into `generated/apply-units/<file stem>/NNNN.tql` and
points `package.json` at the units. It re-splits every listed unit from its source file each time,
so units never keep old contents or an old manifest header, and `ont release` regenerates them
after the bump. `ont validate-package` refuses a unit whose `# manifest:` or `# upstream-tag:`
header names another version.

**`define fun` cannot be repeated.** Defining a function that already exists fails with
`[FUN5] A function with name '…' already exists`. The app redefines only changed functions when it
reapplies a schema; anything that applies a schema file by hand (such as the manual upgrade test)
must leave out unchanged `fun` blocks.

## Traps

**One variable cannot be reused across `put` statements in one pipeline.** Within a pipeline a
variable names one concept in every stage, so `put $module …; put $module …;` with different
patterns fails. Give each statement its own variable, or split into separate queries.

**`put` with a changed non-key attribute makes a second keyed instance.** `put` matches the whole
pattern; when any attribute differs it inserts, and the key (or `@unique`) is violated
(`[CNT9] @unique violated …`). Write keyed records as: `put` on the key alone, then `update` the
other attributes.

```typeql
put $q isa StoredQuestion, has storedQuestionId "lingo/q-01";
update $q has storedQuestionText "Which sources have been read …";
```

**The schema-docs module record is permanent.** The refresh writes `SchemaModule` with
`moduleKey` = package name and `moduleName` = `displayName`. Upgrades never delete that record,
so a changed `displayName` makes the `put` insert a second module with the same key: `[CNT9]
@unique violated … SchemaModule … moduleKey`. Keep `displayName` stable; renaming needs a
migration plan that rewrites the existing record first.

**Role names are unique across a relation hierarchy.** Every relation that specialises one gist
relation (for example all the `sub isCategorizedBy` relations in one database, across packages)
needs role names no other relation in that family uses. Use long, specific role names
(`classifiedSource`, `judgedCheck`), not `subject` or `item`.

**An abstract type's parent must be abstract.** gist's upper types are not marked `@abstract`, so
a package type built on them cannot be abstract either. Owner ruling: leave gist as it is for now;
document that only subtypes are ever written instead.

**Names are shared by every package in the database.** A type, relation, role or attribute name
used by another installed package clashes at install (Lingo's `reference` relation against a
beads attribute). Search the sibling checkouts before naming:
`rg -nw '<Name>' ../ontology-*/schema/*.tql`.

**A dependency's types are not yours.** The app's ownership check refuses any `owns`, parent,
annotation or `plays` in a foreign relation added to a dependency's type. Play roles only in
relations this package declares, or make a subtype.

**Data reapplied on upgrade must be idempotent.** With no migration plan or file, the app reruns
every data file on a database that already holds it. `insert` duplicates; use `put` (key only)
plus `update`.
