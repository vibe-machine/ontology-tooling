---
name: typedb
description: Operational guidance for the TypeDB integration in this workspace. Use when (a) bootstrapping or applying packaged schemas via `PackageBootstrapPlanner` / `SchemaContractValidator` / `TypeQLBatcher` / `PackageMigrationPlanner` in the Ontology plugin; (b) reasoning about the schema/bootstrap registries on disk; or (c) configuring or interpreting TypeDB server logs (Rust `tracing`, `RUST_LOG` modules, `typedb.log`, log rotation, containerized log paths). This skill does NOT cover TypeQL query/schema syntax — for that, load the `typeql` skill. For *where the ontology source repos live* (which `.tql` file lives in which sibling repo), see `ontology-repo-layout`.
---

# TypeDB Operational Guide (Ontology Plugin)

How the One stack actually drives TypeDB at runtime: the bootstrap pipeline, the on-disk registries, and the server's logging.

This is **not** a TypeQL reference. If you need query/schema syntax (`match`, `fetch`, `define`, `insert`, `links`, etc.), load the `typeql` skill — it's maintained from upstream and updated as TypeDB evolves.

---

## 1. Bootstrap Pipeline

The Ontology plugin (`OneApps/Ontology/`) drives TypeDB through a bootstrap pipeline when applying packaged schemas. The stages, in order:

1. **`PackageBootstrapPlanner`** builds a `PackageBootstrapPlan` — an ordered list of `PackageBootstrapUnit`s (each is either a schema `define`/`redefine` or a write query). Order comes from the package's `assembly.loadOrder`, falling back to legacy schema-then-data ordering.
2. **`SchemaContractValidator`** pre-validates the plan before any TypeDB transaction:
   - schema defines declare proper supertypes,
   - write queries reference known types,
   - migration units contain valid `define`/`undefine` statements.

   Failure here means the plan never touches TypeDB.
3. **`TypeQLBatcher`** splits oversized `define`/`redefine` schema queries into transaction-sized batches (default: **≤10 statements or ≤4K chars per batch**). This avoids hitting TypeDB's per-transaction size limits on large schemas.

   **Critical: write queries are NOT batched.** The `ontology-tooling` publisher already shards large write-path files into self-contained apply units under `generated/apply-units/`. Each unit is applied as a single transaction. Re-batching them would break intra-query variable scope.
4. **`PackageMigrationPlanner`** handles version upgrades: reads the `migration` section from `package.json`, resolves migration plans with phases and units, and chains multi-step migrations via BFS when no direct path exists between source and target versions.

### Registries

Two registries persist bootstrap state to JSON in the plugin data directory. Both are actors.

| Registry | File | Tracks |
|---|---|---|
| `SchemaApplyRegistry` | `typedb-schema-registry.json` | Which schemas have been applied to which databases |
| `PackageBootstrapRegistry` | `typedb-package-bootstrap-registry.json` | Bootstrap unit application status per database, including checksums for drift detection |

### When this matters outside the Ontology plugin

When invoking TypeDB from any plugin (not just Ontology), if you need to apply a schema package, route through `PackageBootstrapPlanner` rather than calling `define` queries directly — the validator catches contract violations the raw TypeDB driver won't.

---

## 2. TypeDB Logging

TypeDB uses the Rust `tracing` ecosystem (`tracing` + `tracing-appender` + `tracing-subscriber`), **not** Swift `os.Logger`. Logs go to stdout AND a file simultaneously.

### Where logs go

- **Log directory:** `logs/` relative to the TypeDB executable. Inside an XPC container that's the container's working directory unless overridden.
- **Log file:** `typedb.log` with hourly rotation (timestamped suffixes for archived files).
- **Default level:** `INFO`.
- **Containerized runs:** stdout/stderr is captured by the host; file logs live in `logs/` inside the container.

### Controlling log level

Three knobs, in precedence order:

1. **`RUST_LOG` environment variable** (most useful for containerized TypeDB — pass via `EuroRackModuleDescriptor.environment`):

   ```bash
   RUST_LOG=debug                                           # Overall level
   RUST_LOG=database=trace,server=debug,query=trace,info    # Per-module
   ```

   **Available modules:** `database`, `server`, `storage`, `executor`, `query`, `compiler`, `tonic` (gRPC).

2. **Config file** (`config.yml`):

   ```yaml
   logging:
     directory: "logs"
   ```

3. **CLI argument:**

   ```bash
   typedb --logging.directory /var/log/typedb
   ```

**Precedence:** CLI args > config file > defaults. `RUST_LOG` is independent of the directory config — it controls filter level only, regardless of where logs are written.

### Debugging tips

- Query-level slowness: `RUST_LOG=query=trace,executor=debug,info`.
- Schema apply failures: `RUST_LOG=compiler=debug,database=debug,info`.
- gRPC / driver-side issues: `RUST_LOG=tonic=debug,info`.

---

## See also

- **`typeql`** — TypeQL language reference. Load this whenever you're writing or debugging actual queries (including bootstrap unit `.tql` payloads). The bootstrap pipeline above only orchestrates queries; the query *contents* are TypeQL.
- **`ontology-repo-layout`** — Where the source schemas (`.tql` files, manifests, seed data) actually live, in sibling repos under `/Users/kenpo/SoftwareProjects/collection-vibe-machine/`. Read before editing a schema you found referenced here.
