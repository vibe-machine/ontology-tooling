use std::fs;
use std::path::Path;

use assert_cmd::Command;
use predicates::str::contains;
use tempfile::TempDir;

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
      "prompt_export": "include"
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

fn build_repo() -> TempDir {
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
    write(&corpus.join("negative/missing-relation-roles.json"), NEGATIVE);
    dir
}

#[test]
fn version_json_emits_one_object_to_stdout() {
    let assert = Command::cargo_bin("ont")
        .unwrap()
        .args(["version", "--format", "json"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(parsed["name"], "ont");
    assert!(parsed["library"]["name"].as_str().unwrap() == "vibe-ontology");
}

#[test]
fn version_text_prints_human_summary() {
    Command::cargo_bin("ont")
        .unwrap()
        .args(["version", "--format", "text"])
        .assert()
        .success()
        .stdout(contains("ont"))
        .stdout(contains("library: vibe-ontology"));
}

#[test]
fn corpus_validate_reports_ok_for_valid_corpus() {
    let dir = build_repo();
    let assert = Command::cargo_bin("ont")
        .unwrap()
        .args([
            "corpus",
            "validate",
            "--repo",
            dir.path().to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(parsed["status"], "ok");
    assert_eq!(parsed["counts"]["total"], 2);
    assert_eq!(parsed["counts"]["known_good"], 1);
    assert_eq!(parsed["counts"]["negative"], 1);
}

#[test]
fn corpus_list_filters_by_tag() {
    let dir = build_repo();
    let assert = Command::cargo_bin("ont")
        .unwrap()
        .args([
            "corpus",
            "list",
            "--repo",
            dir.path().to_str().unwrap(),
            "--tag",
            "negative",
            "--format",
            "json",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(parsed["matched"], 1);
    assert_eq!(parsed["items"][0]["id"], "missing-relation-roles");
}

#[test]
fn corpus_validate_fails_on_missing_manifest() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("corpus")).unwrap();
    Command::cargo_bin("ont")
        .unwrap()
        .args(["corpus", "validate", "--repo", dir.path().to_str().unwrap()])
        .assert()
        .failure()
        .stderr(contains("manifest not found"));
}

#[test]
fn completions_bash_writes_to_stdout() {
    let assert = Command::cargo_bin("ont")
        .unwrap()
        .args(["completions", "bash"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("_ont"), "expected completion script for ont, got: {stdout}");
}

#[test]
fn no_subcommand_prints_help() {
    Command::cargo_bin("ont")
        .unwrap()
        .assert()
        .success()
        .stdout(contains("Vibe Machine ontology control surface"))
        .stdout(contains("corpus"))
        .stdout(contains("tui"));
}

#[test]
fn help_includes_all_subcommands() {
    Command::cargo_bin("ont")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("tui"))
        .stdout(contains("corpus"))
        .stdout(contains("version"))
        .stdout(contains("completions"));
}
