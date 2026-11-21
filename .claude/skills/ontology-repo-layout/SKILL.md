---
name: ontology-repo-layout
description: Where the actual ontology source lives, what each repo contains, and when work in OneWorkspace requires crossing into them. Use when adding or looking up an entity type, schema, domain definition, `.tql` file, manifest, or seed data; when a TypeDB query returns nothing because the type isn't defined; when regenerating Swift bindings; when the user mentions any of `ontology`, `schema`, `domain`, `manifest`, `ontology-vibemachine`, `ontology-gist`, `ontology-beads` (the ontology, not the issue tracker), `ontology-gastown`, `ontology-trace-to-knowledge`, `ontology-template`, or `ontology-tooling`. Complements the `typeql` skill (TypeQL query/schema syntax), the `typedb` skill (TypeDB bootstrap pipeline and server logging), and the `vibemachine-ontology` skill (querying the live graph) — this one is about repo geography.
---

# Ontology Repo Layout

The ontology source code lives **outside `OneWorkspace`**. It's a sibling collection of repositories. Plugin code in `OneApps/Ontology/` consumes these repos but does not contain them.

## On-disk location (assume present)

```
/Users/kenpo/SoftwareProjects/
├── collection-one/
│   └── OneWorkspace/                ← this workspace
└── collection-vibe-machine/         ← ontology source lives here
    ├── ontology-template/           ← scaffold for new ontologies
    ├── ontology-tooling/            ← CLI/library that operates on the others
    ├── ontology-vibemachine/        ← domain
    ├── ontology-gist/               ← domain
    ├── ontology-beads/              ← domain (the ontology, not the issue tracker)
    ├── ontology-gastown/            ← domain
    ├── ontology-trace-to-knowledge/ ← domain
    └── TypeDBSwift/                 ← Swift bindings (also vendored under OneWorkspace/Vendor/)
```

Remote: <https://github.com/orgs/vibe-machine/repositories>

These directories are **not git submodules** of OneWorkspace. Treat them as separate sibling checkouts.

## Repo roles

| Repo | What it contains | Typical interaction |
|---|---|---|
| `ontology-template` | Seed/scaffold for new ontology repos. | Read when creating a new domain ontology. |
| `ontology-<domain>` | A single domain's ontology: schema, data, manifests, queries, migrations. | Read to look up types; write to add/change them. |
| `ontology-tooling` | The toolchain that bootstraps, validates, batches, and regenerates against the domain repos. | Run as a CLI; occasionally edit when tooling itself needs a fix. |
| `TypeDBSwift` | Swift bindings to TypeDB. | Read for API shape. Vendored copy under `OneWorkspace/Vendor/TypeDBSwift` is what the build uses; the standalone copy here is for direct development. |

## Per-domain-repo structure

Each `ontology-<domain>` follows the same layout:

```
ontology-<domain>/
├── schema/         ← .tql definitions (entities, relations, attributes, rules)
├── data/           ← seed/example data inserts
├── manifests/      ← package metadata for bootstrap
├── queries/        ← (some repos) reusable TypeQL queries
├── migrations/     ← (some repos) versioned schema migrations
├── tools/          ← repo-local helpers
└── package.json    ← manifest (schemas, dependencies, domains)
```

## When to cross over from OneWorkspace

| Triggering work | Repo to open | Read or write |
|---|---|---|
| A query in OneApp returns nothing because the type isn't defined | `ontology-<domain>/schema/` | **Read** to confirm; **write** to add the type and PR back. |
| Adding a new domain entity / relation / attribute | `ontology-<domain>/schema/` | **Write** — define there, not in OneWorkspace. |
| Adding seed data for a feature or test | `ontology-<domain>/data/` | **Write.** |
| Debugging a bootstrap failure | `ontology-tooling/` and the failing `ontology-<domain>/manifests/` | **Read** first; **write** to either repo if a real bug. |
| Regenerating Swift bindings or other code | `ontology-tooling/` (run the CLI) | **Read/run.** |
| Looking up "is type X defined anywhere?" | `ontology-<domain>/schema/` across all domains | **Read.** Use grepai for cross-repo search. |
| Creating a brand-new domain ontology | `ontology-template/` (clone-and-rename) | **Read** the template; **write** the new repo. |

**Default expectation: agents read AND write.** Edits to ontology repos result in commits in those repos (separate git histories from OneWorkspace), often via PR. There is no auto-generation path that lets you avoid editing the source.

## Tooling

`ontology-tooling/` is the toolchain. It is currently a **Python CLI** (see its `pyproject.toml` and `mise.toml`) and is **migrating to a shared Rust library** so multiple consumers can call into it without subprocess shelling. When invoking tooling commands, always read the repo's current README rather than relying on cached examples — the surface is in flux.

## Searching across the ontology source

Use grepai semantic search across `/Users/kenpo/SoftwareProjects/collection-vibe-machine/` rather than plain grep — many domain concepts are named differently in `.tql` files than in Swift code, and semantic search bridges that gap. Fall back to ast-grep for syntactic patterns within `.tql`.

## Caveat: agent-guidance drift

Several of these repos have their own `CLAUDE.md` and/or `AGENTS.md`. They are **not currently kept in sync** the way `OneWorkspace`'s root files are (symlinked). If the two files exist as separate copies in a domain repo, they may have drifted — read both. Reconciling this is tracked in beads issue `one-egp.1`.

## What this skill does NOT cover

- Writing TypeQL queries → see `typeql` skill.
- Querying the live VibeMachine knowledge graph → see `vibemachine-ontology` skill.
- The Swift `EuroRackContracts` package → see `LocalPackages/EuroRackContracts/CLAUDE.md`.
- The TypeDB bootstrap pipeline (PackageBootstrapPlanner / SchemaContractValidator / TypeQLBatcher) and server logging → that's Swift code under `OneApps/Ontology/`; see the `typedb` skill.
