# The app is the contract: what it reads from a package, and what the tooling checks

The app decides what a package is. `ont validate-package` is a copy of the app's rules, and a copy
can lag. Before adding anything to a package (a field, a file, a convention), find where the app
reads it and how; if the app does not read it, it does not reach the user. Lingo 1.0.2 published
questions in a file format the app never read; Lingo 1.1.1 ran its questions on `fixtures` the app
never loads, so the app had no Run button for them.

App paths are under `OneWorkspace/OneApps/Lingo/`. "Tooling" is `ontology-tooling`
`ont validate-package` (`crates/vibe-ontology/src/package_validator.rs` and the modules it calls)
as of commit f5c4523. Reference packages: gist 5.1.0, beads 0.2.0, Lingo 1.1.2.

## `package.json` fields (`PackageManifest`, `Core/Models/OntologyPackage.swift`)

| Field | What the app does with it | Tooling checks |
|---|---|---|
| `name` | everywhere; names the sample database `sample-<name>` (`PackageSampleDatabase.swift`) | non-empty |
| `displayName` | registry; schema-docs module name, recorded in every installed database | non-empty; keeping it unchanged is your job |
| `version` (semver) | install and upgrade decisions; `moduleVersionKey` | semver and version stamps |
| `description` | registry summary; the decoder fails without it | required |
| `prefix` | type domain tag (`OntologyRuntimeCorpusBuilder.swift`); the decoder fails without it | required |
| `source` | the installed version and installation record are found by `moduleRepoUrl == source`; without it the app cannot tell what is installed or upgrade it | required |
| `author`, `license` | decoded, unused | no |
| `schemas[]`, `data[]`, `manifests[]` | bootstrap plan (`PackageValidator.swift`, `StaticSchemaProvider.swift`) | exist; write-unit limits |
| `provenance` | provenance seeds re-applied on upgrade | exists; stamps |
| `assembly.loadOrder` | install order; every entry must be a declared asset | yes, but see gap 3 |
| `migration` (`format`, `supportsUpgradeFrom`, `plans[]`) | plan choice (`PackageBootstrapPlanner.swift`); plan rules (`PackageValidator.swift`) | `migration_contract.rs`, previous-release coverage, unit limits |
| `dependencies[]` (`name`, `version` range) | load order; version check against what is installed | by name only; see gap 2 |
| `sampleData[]` | builds `sample-<name>` from the package, its dependencies and these files, each applied as **one** write query in its own transaction, in order; never installed into `knowledge`. The Run button on a package's questions appears only when this list is non-empty (`QuestionRunModel.swift`) | `sample_data.rs`: required when questions are declared; distinct, existing, not also in `schemas`/`data`/`loadOrder`; each file one write query within the limits; equal to what `sandbox/sample-data.tql` generates |
| `installation.mode` (`fresh-only`) | read by install and upgrade (`PackageBootstrapPlanner.swift`, `OntologyViewModel+TypeDB.swift`) | no; no package uses it |
| `domains`, `categorization` (legacy) | `StaticSchemaProvider.swift` | exist |

## Files and conventions the app reads

| Convention | Where in the app | Tooling checks |
|---|---|---|
| Stored questions: `schema/stored-queries.tql` exactly as `StoredQuerySchema.typeQL`; `StoredQuestion`/`StoredQuery` records joined by `storedAnswer`, keyed `<package>/...` | `Core/Stores/StoredQuerySchema.swift` | generated from `competency/questions.json` and compared (`stored_questions.rs`) |
| Schema documentation: a write asset whose path contains `schema-docs` or that puts a `SchemaResource`; rebuilt on upgrade | `Core/Models/CrateBootstrapModels.swift` | split apply units, no monolith beside them |
| Provenance: `OntologyModule` with `moduleRepoUrl`, `OntologyModuleVersion` with `moduleVersionKey "<source>@<version>"` | `OntologyViewModel+TypeDB.swift` | stamps; not that `moduleRepoUrl` equals `source` |
| Upgrade: a matching migration plan, else `migrations/v<from>-to-v<to>.tql` as one query, else refused | `PackageBootstrapPlanner.swift` | plans generated; a single file to the package's own version refused |
| Write units: each data file or apply unit is one transaction, never re-batched | `Core/Schema/TypeQLBatcher.swift` | 50 executable blocks, 50,000 characters, 1000 stages; one `match` per upgrade unit |
| Type ownership: no `owns`, parent, annotation or role added to a dependency's type | `Core/Schema/PackageTypeOwnershipCheck.swift` | no; see gap 1 |
| `competencyQuestions`, `queries[]`, `competency/`, `queries/` | **not read by the app**; tooling only. They reach the app only as the generated stored-question files | yes |

## Gaps the tooling still does not check

Only the app's own check (`mise run check-package-release` in OneWorkspace, step 10) catches these.
Check them by hand until the tooling does.

1. **A package must not change a dependency's types.** Adding `owns`, a parent, an annotation, or a
   role in someone else's relation to a type a dependency owns makes the app refuse the install.
   The one allowance: a dependency's type may play a role in a relation this package declares.
2. **Dependency version ranges.** `dependencies[].version` is not checked against the sibling
   checkout the tooling loads; a range the installed dependency does not satisfy passes here and
   fails in the app.
3. **Load-order entries listed only in `generatedArtifacts`.** The tooling accepts an
   `assembly.loadOrder` entry declared only under `assembly.generatedArtifacts`; the app requires
   every load-order entry to be in `schemas` or `data` and refuses the package.

Also open: `installation.mode` is unknown to the tooling; gist and beads declare no
`competencyQuestions`, so today's `validate-package` refuses them.
