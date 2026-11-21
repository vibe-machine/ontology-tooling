# How the app installs and upgrades a package

Read from the app's Lingo plugin (OneWorkspace, `OneApps/Lingo`):
`Core/ViewModels/OntologyViewModel+TypeDB.swift` (`crateBootstrapExecutions`,
`migrationPlan(forSchemaApplication:)`, `applyMigrationPlan`, `applyMigrationDiff`,
`removeSchemaDocumentation`), `Core/Backend/PackageBootstrapPlanner.swift` (`migrationDiff`,
`PackageMigrationPlanner.makePlan`), `Core/Models/OntologyPackage.swift` (`migrationPlan(for:)`),
`Core/Models/CrateBootstrapModels.swift` (`isSchemaDocumentation`),
`Core/Schema/PackageTypeOwnershipCheck.swift`. The tooling's summary is the "Upgrades" section of
`ontology-tooling/README.md`. Re-read them when the behaviour matters; they change.

## Where packages come from

The app clones `vibe-machine/ontology-<name>` from GitHub and installs the newest semver tag.
Only pushed tags exist for it.

## Which version is installed

The installed version is the highest `moduleVersion` of the `OntologyModuleVersion` records
joined to the `OntologyModule` whose `moduleRepoUrl` is the package's `source`. These records come
from the package's provenance data file. The app's local registry is only a cache.

If the installed version equals the package version and an installation record exists for it,
the package is **skipped**. Content pushed under an already-installed version never arrives.

## Before anything is written

- `PackageTypeOwnershipCheck`: every `owns`, `sub`, annotation or `plays` a package states on a
  dependency's type must already be stated by a dependency. The only addition allowed is a
  dependency's type playing a role in a relation this package declares. Otherwise:
  `Can't install <package>: it changes <Type>, which belongs to <owner>.`
- `PackageInstallationPolicy` checks the installed version against the package's `installation`
  policy (`installation.mode` `freshOnly` is checked against existing installation records).

## Fresh install

Schemas in `assembly.loadOrder` order, then data files (schema docs among them), provenance last,
then an `OntologyModuleInstallation` record. Large `define` queries are batched by the app; write
files are not, so each data file or apply unit runs as one transaction and must fit the limits in
`typedb-limits-and-traps.md`.

## Upgrade (installed version lower than the package version)

1. **Choose the path**, in this order:
   - **Migration plan.** `package.json` `migration.plans` has a plan whose `to` is the package
     version and whose `from` range contains the installed version. Its phases run in the order
     written, **each unit in its own transaction**: an `assert-data` unit must return rows, a
     `schema` unit is one schema transaction, a `write` unit is one write query.
   - **Upgrade file.** Else a file in `migrations/` (top level, `v<from>-to-v<to>.tql`) whose
     versions match installed and target exactly, or a chain of such files. Each file is sent as
     **one query in one transaction**, whatever it holds.
   - **Refused.** Else the upgrade stops before writing anything: `no migration plan matches
     installed version <installed> -> <target>`. The data files are not reapplied. (The app
     reapplies data only when it cannot read an installed version and installs as if fresh, or in
     its knowledge-bootstrap reconcile; an author cannot rely on either.)

   A folder `migrations/v<from>-to-v<to>/` of units is never looked inside. Its units run only
   when a plan lists them.
2. A plan runs before the package schemas (after a snapshot when `snapshot.required` is true).
3. **Every schema file is applied again.** `define` only adds, so a removal or rename needs a
   hand-written plan with `undefine`/`redefine` units. The app redefines only changed functions.
4. **Schema documentation is rebuilt.** The app deletes every `SchemaResource` in the
   `SchemaModule` whose `moduleKey` is the package name and applies the schema-docs units again.
   The `SchemaModule` record stays, which is why `displayName` must not change.
5. **Provenance last**, then a new installation record for the version.

A failure in any step stops the upgrade where it is. Earlier transactions are committed, so the
database is left part upgraded.

## Why one file fails: rows multiply

In one query every stage runs once for every row the stages before it produced. A `match` that is
not selected by a key matches every such record, and records without a key accumulate, one set
per installed version (provenance `SourceArtifactRecord`s by `sourcePath`, for example). Chain
several such `match` stages in one file and the row counts multiply: 16 records, then 3, runs the
rest 48 times. Lingo 1.1.0's single file `migrations/v1.0.2-to-v1.1.0.tql` multiplied about
221,000-fold on a database that had lived through several versions, and TypeDB closed the
transaction (`[CXN07] The transaction is closed`). On a fresh database with one earlier version
the same file passed.

## What `ont release` writes

`migration.supportsUpgradeFrom` lists the installed releases the package upgrades from; any other
release is declared not supported (the app refuses it; the user installs fresh). `ont release`
adds the previous release's minor range (`1.0.x`) when no range covers it, and adds a whole
`migration` section when there is none.

For every earlier tag a range covers, `ont release` writes one plan, `"generated": true`
(rewritten on every release; hand-written plans are kept), id `<name>-<from>-to-<to>`:

1. `preflight`: `migrations/preflight/assert-v<from>-module-version.tql` (the source version is
   recorded);
2. `schema`: `migrations/v<from>-to-v<to>-schema.tql`, the package schemas without their
   functions. Defining an installed type changes nothing, so this adds exactly what the release
   adds, before the writes that use it, and is safe to repeat;
3. `migrate`: the write units `migrations/v<from>-to-v<to>/NNNN.tql` (oversized ones re-split
   under `generated/apply-units/migrations/…`), then the provenance files. Only rows changed since
   that release are written, matched by key; schema documentation is left out (the app rebuilds
   it). A changed row whose type has no `@key`/`@unique` is refused with the reason;
4. `verify`: `migrations/verify/assert-v<to>-module-version.tql`.

Worked example: ontology-lingo 1.1.1, three generated plans (from 1.0.1, 1.0.2, 1.1.0),
`supportsUpgradeFrom: [">=1.0.1 <1.2.0"]`, so 1.0.0 and older are declared not supported.
gist 5.1.0 has hand-written plans (`3.0.x`, `4.0.x`, `5.0.x`), each one schema unit.

`ont validate-package` refuses: no plan or `supportsUpgradeFrom` for the previous release; a
covered release no plan upgrades from; a missing `test:typedb-migration` script, or one `test`
does not run; a single-file upgrade to the package's own version; and in any unit the faults
listed in `typedb-limits-and-traps.md`. Single files to earlier versions are released history and
are not re-checked. A first release is exempt.

## Testing upgrades

Two gates, both before pushing. Neither replaces the other.

**`ont qualify --repo .`** (and `ont release`, which runs it in full) ends with
`ont check-upgrade`, on one throwaway TypeDB, a fresh database per case:

- **from each earlier tag on its own**: the dependencies (sibling working trees), that tag, then
  the app's path for that version. A tag outside `supportsUpgradeFrom` with no plan or file is
  reported "not supported, as declared"; a tag inside it must upgrade;
- **from the previous release after an interrupted attempt**: the candidate's schemas already
  applied, as on a database whose upgrade stopped part-way;
- **through every release**: the oldest tag that installs, each later tag upgraded in order, then
  the candidate. Several releases' records sit side by side; this is the case that catches
  row multiplication.

After each, the candidate's module version must be recorded and every question installed as a
`StoredQuestion` with its answer. Each unit has a time limit (`--unit-timeout`, 120 s). A tag
that cannot be installed on today's TypeDB is reported "could not be used as a base" and does not
fail the run. `--quick` runs only the previous release, the interrupted attempt and the
lived-through database. ontology-lingo's full run (eleven tags) takes about 70 s.

**`mise run check-package-release`** (OneWorkspace) runs the same kinds of case through the
app's own Install / Upgrade code, and `--from-backup` adds an upgrade of a restored copy of the
app's newest `knowledge` backup (written, not yet exercised). See SKILL.md step 10 and
`OneApps/Lingo/CLAUDE.md` "Before Releasing a Package".

**What the earlier test got wrong.** The Lingo 1.1.0 upgrade test started from a fresh database
with one earlier version, and split the upgrade file into separate queries. The app does neither:
it runs the file as one query, on databases that have lived through many versions. Test the way
the app runs, on the data the app has.
