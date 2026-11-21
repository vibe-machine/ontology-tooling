# How the app installs and upgrades a package

Read from the app's Lingo plugin (OneWorkspace, `OneApps/Lingo`):
`Core/ViewModels/OntologyViewModel+TypeDB.swift` (`crateBootstrapExecutions`, the package
execution loop, `applyMigrationDiff`, `removeSchemaDocumentation`),
`Core/Backend/PackageBootstrapPlanner.swift` (`migrationDiff`, `PackageMigrationPlanner`),
`Core/Models/OntologyPackage.swift` (`migrationPlan(for:)`),
`Core/Models/CrateBootstrapModels.swift` (`isSchemaDocumentation`),
`Core/Schema/PackageTypeOwnershipCheck.swift`. Re-read them when the behaviour matters; they
change.

## Where packages come from

The app clones `vibe-machine/ontology-<name>` from GitHub and installs the newest semver tag.
Only pushed tags exist for it.

## Which version is installed

The installed version is the highest `moduleVersion` of the `OntologyModuleVersion` records
joined to the `OntologyModule` whose `moduleRepoUrl` is the package's `source`. These records come
from the package's provenance data file. The app's local registry is only a cache; without live
provenance it is discarded.

If the installed version equals the package version and an installation record exists for it,
the package is **skipped**. Content pushed under an already-installed version never arrives.

## Before anything is written

- `PackageTypeOwnershipCheck`: for each package, every `owns`, `sub`, annotation or `plays` it
  states on a type that a dependency declares must already be stated by a dependency. The only
  addition allowed is a dependency's type playing a role in a relation this package declares.
  Otherwise: `Can't install <package>: it changes <Type>, which belongs to <owner>.`
- `PackageInstallationPolicy` checks the installed version against the package's `installation`
  policy; a package declaring `installation.mode` `freshOnly` is checked against existing
  installation records before anything is written.

## Fresh install

Schemas in `assembly.loadOrder` order, then data files (schema docs among them), provenance last,
then an `OntologyModuleInstallation` record. Large `define` queries are batched by the app; write
files are not, so each data file or apply unit runs as one transaction and must fit the limits in
`typedb-limits-and-traps.md`.

## Upgrade (installed version lower than the package version)

1. **Choose the path.**
   - A **migration plan** is used when `package.json` `migration.plans` has a plan whose `to`
     equals the package version exactly and whose `from` range contains the installed version.
   - Otherwise a **migration file** is looked for in `migrations/` (top level only, name
     `v<from>-to-v<to>.tql`): first one whose versions match installed and target exactly; failing
     that, a chain of two or more such files found by breadth-first search
     (`v1.0.0-to-v1.0.1.tql`, `v1.0.1-to-v1.0.2.tql`).
   - Otherwise neither: the data files are reapplied in full.
2. **Migration plan phases run first**, before any schema, in the order written (after a snapshot
   when `snapshot.required` is true). Use a plan for anything a `define` cannot do: `undefine`,
   `redefine`, re-parenting, moving data.
3. **Every schema file is applied again.** `define` only adds, so a removal or rename needs a plan.
4. **Data:**
   - migration file: sent as **one write query** (each step of a chain as one query). It must
     stay under 1000 pipeline stages (`QEX18`).
   - neither plan nor file: every data file except provenance is reapplied, so each must be safe
     to run on a database that already holds it.
   - migration plan: the package's data files are **not** reapplied. A plan that needs data
     written must list those files as `write` units in a phase (as ontology-beads does with its
     categories and questions).
5. **Schema documentation is rebuilt.** On any upgrade the app deletes every `SchemaResource` in
   the `SchemaModule` whose `moduleKey` is the package name, with their links, and applies the
   schema-docs units again. The `SchemaModule` record itself stays, which is why `displayName`
   (its `moduleName`) must not change. A unit counts as schema documentation when its path
   contains `schema-docs` or it writes `SchemaResource` records.
6. **Provenance last**, then a new installation record for the version.

A failure in any step stops the upgrade where it is. Earlier steps are committed, so the database
is left part upgraded. That is why the upgrade is tested before a push.

## What `ont release` writes for an upgrade

When data changed since the previous tag, `ont release` writes the changed rows (schema
documentation left out) to `migrations/v<prev>-to-v<next>.tql` when it fits in one query.
When it does not, it writes ordered units `migrations/v<prev>-to-v<next>/NNNN.tql`.

**Current limit:** the app does not look inside such a folder. The units run only when a
migration plan lists them (`ont release` rewrites an existing plan's `write` unit of that name into
the units). A package with no `migration` section and an oversized upgrade therefore falls back
to reapplying its data files in full, so those files must be safe to run twice. Lingo 1.0.1
shipped that way on purpose, by removing the oversized file.

## Testing an upgrade by hand

No command runs the upgrade path yet; `mise run check-package-release` installs fresh only. The
method below worked for Lingo 1.0.2 to 1.1.0. It drives TypeDB directly, in the order the app
uses, and is **not** the app's install path: it proves the files, not the app's handling of them.
Use a throwaway TypeDB server, never the owner's database.

1. Check out the previous release beside the candidate:
   `git worktree add ../ontology-<name>-v<prev> v<prev>`.
2. Create an empty database. Load gist: its schema files, then its data files, in gist's
   `assembly.loadOrder`.
3. Load the previous release from the worktree the same way: schemas, then data, in its
   `assembly.loadOrder`. This is the "installed" database.
4. Apply the candidate's schema files (each in a schema transaction).
5. Run `migrations/v<prev>-to-v<next>.tql` as **one** write transaction, as the app does.
6. Read back the module versions and a count that proves the new content. Lingo 1.1.0: the
   module versions query (step 11 of the skill) shows `1.1.0`, and 27 `StoredQuestion` records
   each with a `storedAnswer`:

   ```typeql
   match $q isa StoredQuestion, has storedPackage "lingo";
     storedAnswer (question: $q, query: $a);
   reduce $n = count;
   ```

7. Remove the worktree: `git worktree remove ../ontology-<name>-v<prev>`.

**Trap:** applying the candidate's schema file raw fails with
`[FUN5] A function with name '…' already exists` when the package's functions did not change,
because a plain `define fun` cannot be repeated. The app handles this itself (it redefines only
changed functions). For the manual test, apply the schema without the unchanged `fun` blocks.

Passing: every step succeeds, the newest module version is the candidate, and the count matches
the candidate's content. A failure in step 5 is the `QEX18` kind of failure that left a real
database half upgraded; fix it before pushing.
