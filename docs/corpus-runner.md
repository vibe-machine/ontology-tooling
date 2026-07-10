# Corpus Runner

`ont` is the unified Vibe Machine CLI/TUI. Its `corpus` subcommands are the
shared command surface ontology repos use to validate executable corpora and
to export prompt-ready examples for downstream consumers (Lingo, OneApp,
TypeQL prompt fixtures).

The durable implementation lives in the `vibe-ontology` library crate
(`crates/vibe-ontology/`) so that other Vibe Machine products can embed corpus
discovery, validation, and export without depending on clap or ratatui.

## v0 command surface

```bash
ont corpus list     --repo <path> [--tag <t>] [--item <id>] [--format json|text]
ont corpus validate --repo <path>                              [--format json|text]
```

`ont corpus list` discovers the corpus, applies the supplied filters, and
prints a summary plus per-item record. `ont corpus validate` performs the same
discovery but reports only counts and a `status: "ok"` if every item passed
shape validation.

The current v0 executor is **shape-only**: it validates the manifest, fixture
references, and corpus item shape. It does not yet run TypeQL queries against
TypeDB. `ont corpus generate` and TypeDB-backed execution are tracked as
follow-up work; this surface is stable enough for ontology repos to wire
`mise run corpus:test` from their own mise tasks today.

## Output discipline

- **stdout** carries data only.
  - `--format json` (default) emits a single JSON object plus trailing newline.
  - `--format text` emits a short human summary.
- **stderr** carries `tracing` progress and error messages.
- The process exits non-zero on any validation or runtime failure.

This makes the same invocation safe inside agents, mise tasks, and CI: the
JSON on stdout is parseable without filtering progress noise out of stderr.

## Expected ontology repo layout

```
<ontology-repo>/
  corpus/
    manifest.json
    fixtures/
      <fixture-id>-schema.tql
      <fixture-id>-data.tql
    queries/
      <topic>.json          # known-good examples
    negative/
      <topic>.json          # malformed / repair examples (kept separate)
```

Each `.json` file under `queries/` or `negative/` may be a single item, an
array of items, or `{ "items": [...] }`.

### Manifest shape

```json
{
  "version": 1,
  "ontology_package": "gist",
  "fixtures": [
    {
      "id": "meeting-decisions",
      "schema": "fixtures/meeting-decisions-schema.tql",
      "data": "fixtures/meeting-decisions-data.tql"
    }
  ]
}
```

### Item shape

```json
{
  "id": "meeting-decisions-list",
  "title": "Meetings that produced decisions",
  "natural_language_intent": "Find meetings whose activities produced a decision content node.",
  "query_kind": "read",
  "ontology_tags": ["gist", "activity", "content"],
  "fixtures": ["meeting-decisions"],
  "typeql": "match $m isa Meeting; $d isa Decision; ($m, $d) isa produced_decision; fetch $m, $d;",
  "expected": { "kind": "non_empty" },
  "prompt_export": "include",
  "provenance": "lingo-failure-2026-04-01"
}
```

- `query_kind` ∈ `schema | read | write | fetch | reduce`
- `prompt_export` ∈ `include | exclude | bad_to_good`
- `expected.kind` ∈ `row_count | non_empty | empty | json_shape`
  - `row_count` requires a non-negative integer `value`
  - `json_shape` requires a `shape` payload
- `fixtures[]` must reference manifest fixture ids
- Item ids must be unique across `queries/` and `negative/`

## Integration from ontology repos

Ontology repos add thin mise tasks that delegate to `ont`:

```toml
# ontology-gist/mise.toml
[tasks."corpus:test"]
run = "ont corpus validate --repo ."

[tasks."corpus:list"]
run = "ont corpus list --repo ."
```

Once `ont corpus generate` and TypeDB-backed execution land, the same surface
will gain `ont corpus generate` and `ont corpus run` without breaking the
above.

## Library consumers

External products that need corpus discovery/validation without the CLI:

```toml
[dependencies]
vibe-ontology = { path = "../ontology-tooling/crates/vibe-ontology", version = "0.1" }
```

Public surface today:

```rust
use vibe_ontology::corpus::{
    discover_corpus, select_exportable_items, ItemFilter, PromptExportArtifact,
    DEFAULT_EXPORT_PATH,
};
use vibe_ontology::Error;

let corpus = discover_corpus("./ontology-gist")?;
let exportable = select_exportable_items(&corpus.items, None);
let artifact = PromptExportArtifact::new(&corpus.manifest, exportable);
```

`vibe-ontology` deliberately exposes no clap, ratatui, or terminal types.

## Boundary reminder

This crate owns:

- the `ont corpus` command surface
- corpus manifest/item shape validation
- the prompt-export artifact schema and default path
- the embeddable Rust library other Vibe Machine products consume

This crate does **not** own:

- corpus content (lives in ontology repos, e.g. `ontology-gist/corpus/`)
- TypeDB query execution (deferred to follow-up beads)
- prompt template construction (Lingo / `one-537.13`)
- release orchestration (handled by `ont release`, invoked as
  `bin/ont release`)
