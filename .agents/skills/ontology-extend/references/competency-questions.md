# Competency questions: files, format, sample data and generated output

Worked example: `ontology-lingo` at v1.1.2 (`competency/questions.json` with 27 questions,
`queries/`, `sandbox/sample-data.tql` split into `sandbox/sample-data/0001.tql` and `0002.tql`,
and the generated `docs/competency-questions.md`, `schema/stored-queries.tql`,
`data/lingo-questions.tql`). `ontology-beads` ships the same installed form and numbered sample
files. The rules live in `ontology-tooling` (`crates/vibe-ontology/src/competency_questions.rs`,
`sample_data.rs`, `stored_questions.rs`) and its README. What the app reads:
[app-package-contract.md](app-package-contract.md).

## Declaration in package.json

```json
"competencyQuestions": "competency/questions.json",
"sampleData": ["sandbox/sample-data/0001.tql", "sandbox/sample-data/0002.tql"]
```

`competencyQuestions` may instead be `{"waived": "<reason>"}` for a package that deliberately has
none. One of the two is required. Declared questions also require a non-empty `sampleData`:
`ont validate-package` refuses without it, because the app runs questions only on sample data.

## competency/questions.json (formatVersion 1)

```json
{
  "formatVersion": 1,
  "questions": [
    {
      "id": "q01",
      "number": 1,
      "group": "What was read",
      "text": "Which sources have been read for a subject, by whom, and when?",
      "title": "Readings for a subject",
      "query": "queries/q01-readings-for-a-subject.tql",
      "explanation": "Finds the readings done for one subject and shows the source, the reader and the date."
    }
  ]
}
```

- `id` unique, `number` a unique positive integer, `group`, `text` and `query` required.
- `title` (short name of the query) and `explanation` (one or two plain sentences on how the query
  answers the question) are optional but are what the app shows; write them.
- `expectedEmpty` (optional, a non-empty reason): the question is meant to find nothing on the
  sample data. It then passes only with zero rows. Every other question **fails** on zero rows
  ("add sample records it should find, or mark the question expectedEmpty with the reason").
- `query` is a file holding exactly one read query. Every `.tql` under `queries/` must be used by
  a question or listed in `package.json` `queries`.
- The old `fixtures` list is gone. A questions file that still has it is refused
  ("'fixtures' is no longer read"); move the records into sample data.

`docs/competency-questions.md` is generated from this file; edit the JSON, not the Markdown.

## Sample data

The app builds a database `sample-<package>` from the package, its dependencies and the
`sampleData` files, and shows a Run button on the package's questions only when `sampleData` is
non-empty. `ont check-questions` loads the same files the same way, so the questions run on
exactly what the app runs them on. Sample data is invented, in our own words, and never installed.

- Write it in one source, `sandbox/sample-data.tql`, with queries separated by lines reading
  `end;`. Comment lines before a query stay with it.
- `ont prepare-package` (and so `ont release`) splits it into `sandbox/sample-data/NNNN.tql`, each
  headed "Generated from sandbox/sample-data.tql", removes numbered files it no longer makes, and
  sets `sampleData`. **Never split or edit the numbered files by hand**; validation refuses files
  that differ from what the source generates.

The app applies each file as **one** write query in its own transaction, so `ont validate-package`
refuses a sample file that:

- is listed twice, does not exist, or is also in `schemas`, `data` or `assembly.loadOrder`;
- holds an `end;` line (more than one query), is a schema query (`define`, `undefine`,
  `redefine`), or writes nothing (no `insert`, `put`, `update` or `delete`);
- holds more than one `match` (every later stage runs once per row it found, so rows multiply);
- exceeds 1000 stages, 50 executable blocks or 50,000 characters.

For too many `match` stages or a file over the limits, split it into more queries in the source
with `end;` lines and run `ont prepare-package --repo .`.

## Running them

```bash
../ontology-tooling/bin/ont check-questions --repo .                    # run
../ontology-tooling/bin/ont check-questions --repo . --write-generated  # regenerate files only
```

A run starts a throwaway TypeDB (`TYPEDB_BIN`, `--typedb-bin`, or `typedb` on `PATH`), loads the
dependencies from the sibling `ontology-<name>` checkouts, the package in `assembly.loadOrder`,
then each `sampleData` file as one write query, and runs each query. Each line prints the row
count; read them. The run also checks that the package installs exactly one `StoredQuestion` per
question, each answered by a `StoredQuery`.

On success it writes `competency/last-run.json` with a digest of the questions file, the queries,
the sample-data files and the package's own schema and data files. A schema, data or sample change
therefore calls for a fresh run; `ont release --validate-only` refuses when the committed digest
does not match. `ont release` runs the questions itself on exactly what ships.

## What reaches the app

The app shows a package's questions only when they are installed in its database as records.
The questions file is turned into two installed, generated files:

- `schema/stored-queries.tql`: exactly the schema the app declares in
  `OneApps/Lingo/Core/Stores/StoredQuerySchema.swift` (`StoredQuestion`, `StoredQuery`, the
  `storedAnswer` relation and their `stored*` attributes, with no gist parent). Listed in
  `schemas` as `stored-queries`. Several packages may declare it; the text is identical.
- `data/<package>-questions.tql`: per question one `StoredQuery` keyed
  `<package>/<slug of the title>` (title, query text without leading comments, package, version,
  explanation) and one `StoredQuestion` keyed `<package>/q-NN` (text, package, version, order),
  joined by `storedAnswer`. Each record is `put` on its key and its other attributes set with
  `update`, so a reinstall or upgrade leaves one record per key. `storedVersion` is the version
  at which the entry last changed.

Register both files once in `schemas`, `data` and `assembly.loadOrder` (stored-queries schema
before the package schema; questions data with the other data). `ont validate-package` fails when
either is missing or differs from what would be generated. Questions kept only as repository
files never reach the app; questions without `sampleData` reach it but cannot be run.
