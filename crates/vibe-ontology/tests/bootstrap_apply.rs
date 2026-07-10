//! Live integration test for applying a full ontology package bootstrap
//! (schema + put-based data) to a fresh TypeDB server (one-2xg.46.5).
//!
//! This is the Rust replacement for the former Python suite
//! (tests/test_large_write_regression.py, test_schema_apply.py,
//! test_upgrade_bootstrap.py). It re-expresses their meaningful integration
//! invariants as self-contained assertions against a locally-spun server —
//! no remote VM, no `collection-one` package tree, no Python driver:
//!
//!   1. The NORMALIZED gist v2.0.0 bootstrap (after `prepare_executable_package`
//!      splits the oversized `gistCore-schema-docs` write) applies cleanly to a
//!      fresh server and does NOT crash it (test_large_write_regression).
//!   2. Re-applying the same bootstrap is idempotent — `define` is additive and
//!      `put`-based data does not raise @unique/@key violations (test_schema_apply).
//!   3. Delete-all-types → re-apply reproduces the Cannon UI flow and must not
//!      raise a @key violation on the second bootstrap (test_upgrade_bootstrap).
//!
//! Sourced from the sibling `ontology-gist` repo at tag v2.0.0 (the same source
//! the Python test used). Requires that repo present and a `typedb` binary
//! (env `TYPEDB_BIN`); it fails loudly rather than skipping.

use std::path::{Path, PathBuf};
use std::process::Command;

use vibe_ontology::apply::{
    apply_schema, apply_write, create_database, delete_database, TypeDbTarget,
};
use vibe_ontology::executable_package::prepare_executable_package;

mod common;
use common::TypeDbServer;

const GIST_REF: &str = "v2.0.0";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn normalized_gist_bootstrap_applies_idempotently_to_fresh_typedb() {
    let mut server = TypeDbServer::start();
    let target = TypeDbTarget::local(server.address.clone());
    let database = "gist_bootstrap_test";

    // Source gist v2.0.0 into a temp dir and normalize it (splits the oversized
    // schema-docs write into safe executable apply units, rewrites loadOrder).
    let workdir = tempfile::tempdir().expect("temp workdir");
    let package_dir = workdir.path().join("gist");
    archive_gist(GIST_REF, &package_dir);
    prepare_executable_package(&package_dir, None).expect("prepare executable package");

    let load_order = read_load_order(&package_dir);
    assert!(
        !load_order.is_empty(),
        "normalized gist v2.0.0 package.json has an empty assembly.loadOrder"
    );

    // Round 1 — fresh apply. The oversized schema-docs write must be split so
    // this does not overflow the server's stack / kill the process.
    create_database(&target, database).await.expect("create db");
    apply_bootstrap(&target, database, &package_dir, &load_order)
        .await
        .expect("round 1: normalized bootstrap must apply to a fresh server");
    assert!(
        server.is_alive(),
        "TypeDB server crashed during the first normalized gist bootstrap apply"
    );

    // Round 2 — idempotent re-apply. define is additive; put-based data must not
    // raise @unique / @key violations on a second application.
    apply_bootstrap(&target, database, &package_dir, &load_order)
        .await
        .expect(
            "round 2: re-applying the bootstrap must be idempotent (no @unique/@key violation)",
        );
    assert!(server.is_alive(), "server died on idempotent re-apply");

    // Round 3 — "Delete All Types" → re-apply (the Cannon UI flow). A fresh DB
    // re-applied in the same order must not raise a @key violation.
    delete_database(&target, database).await.expect("delete db");
    create_database(&target, database)
        .await
        .expect("recreate db");
    apply_bootstrap(&target, database, &package_dir, &load_order)
        .await
        .expect("round 3: delete-all → re-apply must not raise a @key violation");
    assert!(server.is_alive(), "server died on delete-then-reapply");

    server.stop();
}

/// Applies every load-order asset in order, choosing the transaction type from
/// the query text (mirrors the app's `transactionType(for:)`).
async fn apply_bootstrap(
    target: &TypeDbTarget,
    database: &str,
    package_dir: &Path,
    load_order: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    for asset in load_order {
        let path = package_dir.join(asset);
        let query = std::fs::read_to_string(&path)
            .map_err(|e| format!("read load-order asset {}: {e}", path.display()))?;
        if query.trim().is_empty() {
            continue;
        }
        if is_schema_query(&query) {
            apply_schema(target, database, &[query.as_str()]).await?;
        } else {
            apply_write(target, database, &query).await?;
        }
    }
    Ok(())
}

/// A load-order asset is schema iff it contains a top-level `define`/`undefine`/
/// `redefine` operation; otherwise it is put/insert-based data (a write).
fn is_schema_query(query: &str) -> bool {
    query
        .lines()
        .map(str::trim)
        .any(|line| matches!(line, "define" | "undefine" | "redefine"))
}

fn read_load_order(package_dir: &Path) -> Vec<String> {
    let manifest_path = package_dir.join("package.json");
    let text = std::fs::read_to_string(&manifest_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", manifest_path.display()));
    let manifest: serde_json::Value =
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse package.json: {e}"));
    manifest["assembly"]["loadOrder"]
        .as_array()
        .unwrap_or_else(|| panic!("package.json has no assembly.loadOrder array"))
        .iter()
        .map(|v| {
            v.as_str()
                .unwrap_or_else(|| panic!("loadOrder entry is not a string: {v}"))
                .to_string()
        })
        .collect()
}

/// Extracts `git archive <ref>` of the sibling ontology-gist repo into `dest`.
fn archive_gist(git_ref: &str, dest: &Path) {
    let gist = gist_repo();
    std::fs::create_dir_all(dest).expect("create package dir");

    let tar_path = dest.with_extension("tar");
    let status = Command::new("git")
        .current_dir(&gist)
        .args([
            "archive",
            "--format=tar",
            "--output",
            tar_path.to_str().unwrap(),
            git_ref,
        ])
        .status()
        .expect("run git archive");
    assert!(status.success(), "git archive {git_ref} failed");

    let status = Command::new("tar")
        .args([
            "-xf",
            tar_path.to_str().unwrap(),
            "-C",
            dest.to_str().unwrap(),
        ])
        .status()
        .expect("run tar -x");
    assert!(status.success(), "tar extract failed");
    let _ = std::fs::remove_file(&tar_path);
}

/// Locates the sibling `ontology-gist` repo (`<tooling>/../ontology-gist`).
fn gist_repo() -> PathBuf {
    // CARGO_MANIFEST_DIR = <tooling>/crates/vibe-ontology
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../ontology-gist")
        .canonicalize();
    match repo {
        Ok(path) if path.join(".git").exists() => path,
        _ => panic!(
            "sibling ontology-gist repo not found next to ontology-tooling; \
             this bootstrap test sources gist {GIST_REF} from it and must not silently skip"
        ),
    }
}
