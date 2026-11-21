# Competency questions: files, format and generated output

Worked example: `ontology-lingo` at v1.1.0 (`competency/questions.json` with 27 questions,
`queries/`, `fixtures/`, and the generated `docs/competency-questions.md`,
`schema/stored-queries.tql`, `data/lingo-questions.tql`). `ontology-beads` ships the same
installed form. The rules live in `ontology-tooling`
(`crates/vibe-ontology/src/competency_questions.rs`, `stored_questions.rs`) and its README.

## Declaration in package.json

```json
"competencyQuestions": "competency/questions.json"
```

or, for a package that deliberately has none, `{"waived": "<reason>"}`. One of the two is
required; `ont validate-package` fails without it.

## competency/questions.json (formatVersion 1)

```json
{
  "formatVersion": 1,
  "fixtures": ["fixtures/sample.tql", "fixtures/more-sample.tql"],
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
- `query` is a file holding exactly one read query. Every `.tql` under `queries/` must be used by
  a question or listed in `package.json` `queries`.
- `fixtures` load in order after the package. They are invented sample data, written in our own
  words, and are never listed in `data` or `assembly.loadOrder`.

`docs/competency-questions.md` is generated from this file; edit the JSON, not the Markdown.

## Running them

```bash
../ontology-tooling/bin/ont check-questions --repo .                    # run
../ontology-tooling/bin/ont check-questions --repo . --write-generated  # regenerate files only
```

A run starts a throwaway TypeDB (`TYPEDB_BIN`, `--typedb-bin`, or `typedb` on `PATH`), loads the
dependencies from the sibling `ontology-<name>` checkouts, the package in `assembly.loadOrder`,
then the fixtures, and runs each query. A question passes when its query runs and returns at
least one row. The run also checks that the package installs exactly one `StoredQuestion` per
question, each answered by a `StoredQuery`.

On success it writes `competency/last-run.json` with a digest of the questions file, the queries,
the fixtures and the package's own schema and data files. A schema or data change therefore calls
for a fresh run; `ont release --validate-only` refuses when the committed digest does not match.
`ont release` runs the questions itself on exactly what ships and commits the new record.

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
files never reach the app.
