use std::fs;
use std::path::Path;

use tempfile::TempDir;
use vibe_ontology::corpus::{
    discover_corpus, select_exportable_items, ItemCategory, ItemFilter, PromptExportArtifact,
};
use vibe_ontology::Error;

const KNOWN_GOOD: &str = r#"{
  "items": [
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
    },
    {
      "id": "decisions-list",
      "title": "All decisions",
      "natural_language_intent": "List all decisions across the corpus.",
      "query_kind": "fetch",
      "ontology_tags": ["gist", "content"],
      "fixtures": ["meeting-decisions"],
      "typeql": "match $d isa Decision; fetch $d;",
      "expected": { "kind": "non_empty" },
      "prompt_export": "exclude"
    }
  ]
}"#;

const NEGATIVE: &str = r#"{
  "id": "missing-relation-roles",
  "title": "Relation without explicit roles",
  "natural_language_intent": "Demonstrate a malformed relation pattern with missing roles.",
  "query_kind": "read",
  "ontology_tags": ["gist", "negative"],
  "fixtures": ["meeting-decisions"],
  "typeql": "match $r ($a, $b) isa produced_decision; fetch $r;",
  "expected": { "kind": "row_count", "value": 0 },
  "prompt_export": "bad_to_good"
}"#;

const MANIFEST: &str = r#"{
  "version": 1,
  "ontology_package": "fixture-ontology",
  "fixtures": [
    {
      "id": "meeting-decisions",
      "schema": "fixtures/meeting-decisions-schema.tql",
      "data": "fixtures/meeting-decisions-data.tql"
    }
  ]
}"#;

fn write(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, body).unwrap();
}

fn build_corpus_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    let corpus = repo.join("corpus");
    write(&corpus.join("manifest.json"), MANIFEST);
    write(
        &corpus.join("fixtures/meeting-decisions-schema.tql"),
        "define entity Meeting; entity Decision;\n",
    );
    write(
        &corpus.join("fixtures/meeting-decisions-data.tql"),
        "insert $m isa Meeting; $d isa Decision;\n",
    );
    write(&corpus.join("queries/meeting-decisions.json"), KNOWN_GOOD);
    write(
        &corpus.join("negative/missing-relation-roles.json"),
        NEGATIVE,
    );
    dir
}

#[test]
fn discover_loads_manifest_fixtures_queries_and_negative_items() {
    let dir = build_corpus_repo();
    let corpus = discover_corpus(dir.path()).unwrap();
    assert_eq!(corpus.manifest.ontology_package, "fixture-ontology");
    assert_eq!(corpus.manifest.fixtures.len(), 1);
    assert_eq!(corpus.items.len(), 3);

    let categories: Vec<_> = corpus.items.iter().map(|i| i.category).collect();
    assert!(
        categories
            .iter()
            .filter(|c| **c == ItemCategory::KnownGood)
            .count()
            == 2
    );
    assert!(
        categories
            .iter()
            .filter(|c| **c == ItemCategory::Negative)
            .count()
            == 1
    );
}

#[test]
fn discover_rejects_unknown_fixture_reference() {
    let dir = build_corpus_repo();
    let bad = KNOWN_GOOD.replace("\"meeting-decisions\"", "\"does-not-exist\"");
    write(
        &dir.path().join("corpus/queries/meeting-decisions.json"),
        &bad,
    );
    let err = discover_corpus(dir.path()).unwrap_err();
    assert!(matches!(err, Error::Invalid(ref msg) if msg.contains("unknown fixture id")));
}

#[test]
fn discover_rejects_invalid_query_kind() {
    let dir = build_corpus_repo();
    let bad = KNOWN_GOOD.replace("\"query_kind\": \"read\"", "\"query_kind\": \"bogus\"");
    write(
        &dir.path().join("corpus/queries/meeting-decisions.json"),
        &bad,
    );
    let err = discover_corpus(dir.path()).unwrap_err();
    assert!(
        matches!(err, Error::Json { .. }),
        "expected Json deserialization error, got {err:?}"
    );
}

#[test]
fn discover_rejects_duplicate_item_ids() {
    let dir = build_corpus_repo();
    write(&dir.path().join("corpus/queries/duplicate.json"), NEGATIVE);
    write(
        &dir.path()
            .join("corpus/negative/missing-relation-roles.json"),
        NEGATIVE,
    );
    let err = discover_corpus(dir.path()).unwrap_err();
    assert!(matches!(err, Error::Invalid(ref msg) if msg.contains("duplicate corpus item id")));
}

#[test]
fn discover_rejects_missing_fixture_file() {
    let dir = build_corpus_repo();
    fs::remove_file(
        dir.path()
            .join("corpus/fixtures/meeting-decisions-data.tql"),
    )
    .unwrap();
    let err = discover_corpus(dir.path()).unwrap_err();
    assert!(matches!(err, Error::Invalid(ref msg) if msg.contains("missing data file")));
}

#[test]
fn discover_rejects_missing_corpus_directory() {
    let dir = TempDir::new().unwrap();
    let err = discover_corpus(dir.path()).unwrap_err();
    assert!(matches!(err, Error::CorpusMissing(_)));
}

#[test]
fn discover_rejects_missing_manifest() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("corpus")).unwrap();
    let err = discover_corpus(dir.path()).unwrap_err();
    assert!(matches!(err, Error::ManifestMissing(_)));
}

#[test]
fn item_filter_matches_by_tag_and_id() {
    let dir = build_corpus_repo();
    let corpus = discover_corpus(dir.path()).unwrap();

    let by_tag = ItemFilter {
        tag: Some("activity".into()),
        ..Default::default()
    };
    let activity_items: Vec<_> = corpus.items.iter().filter(|i| by_tag.matches(i)).collect();
    assert_eq!(activity_items.len(), 1);
    assert_eq!(activity_items[0].id, "meeting-decisions-list");

    let by_id = ItemFilter {
        item_id: Some("decisions-list".into()),
        ..Default::default()
    };
    let id_items: Vec<_> = corpus.items.iter().filter(|i| by_id.matches(i)).collect();
    assert_eq!(id_items.len(), 1);
    assert_eq!(id_items[0].id, "decisions-list");
}

#[test]
fn select_exportable_excludes_negative_and_excluded_items() {
    let dir = build_corpus_repo();
    let corpus = discover_corpus(dir.path()).unwrap();
    let exportable = select_exportable_items(&corpus.items, None);
    assert_eq!(exportable.len(), 1);
    assert_eq!(exportable[0].id, "meeting-decisions-list");
}

#[test]
fn prompt_export_artifact_carries_manifest_metadata() {
    let dir = build_corpus_repo();
    let corpus = discover_corpus(dir.path()).unwrap();
    let exportable = select_exportable_items(&corpus.items, None);
    let artifact = PromptExportArtifact::new(&corpus.manifest, exportable);
    assert_eq!(artifact.schema_version, 1);
    assert_eq!(artifact.ontology_package, "fixture-ontology");
    assert_eq!(artifact.source_corpus_version, 1);
    assert_eq!(artifact.item_count, 1);
}
