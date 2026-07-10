//! Live integration test for the schema apply engine (one-2i7 / one-2xg.46).
//!
//! Installs the REAL gist 2.0.4 schema into a real TypeDB server, then applies
//! the REAL gist 3.0.0 structural-property-hierarchy migration. The app's
//! Swift/C-driver path fails this exact migration with `SVL37` (it validates
//! each schema query eagerly); this engine applies it via one schema
//! transaction with commit-time validation — the same mechanism TypeDB console
//! uses — and must succeed.
//!
//! Requires a `typedb` binary (env `TYPEDB_BIN`, else `typedb` on PATH, else
//! the Homebrew location). If none is found the test fails loudly rather than
//! silently passing — a live apply test that skips is worthless.

use std::path::PathBuf;

use vibe_ontology::apply::{apply_schema, apply_schema_migration, create_database, TypeDbTarget};

mod common;
use common::TypeDbServer;

const GIST_2_0_4_LOAD_ORDER: &[&str] = &[
    "self-describing-schema.tql",
    "gistCore.tql",
    "package-provenance.tql",
    "administrative-operations.tql",
];

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn applies_real_gist_2_0_4_to_3_0_0_mixed_schema_migration() {
    let mut server = TypeDbServer::start();
    let target = TypeDbTarget::local(server.address.clone());
    let database = "gist_upgrade_test";

    create_database(&target, database)
        .await
        .expect("create database");

    // 1. Install real gist v2.0.4 (one schema transaction, load order).
    let schema_blobs: Vec<String> = GIST_2_0_4_LOAD_ORDER
        .iter()
        .map(|name| read_fixture(&format!("gist-2.0.4-schema/{name}")))
        .collect();
    apply_schema(&target, database, &schema_blobs)
        .await
        .expect("gist 2.0.4 install");

    // 2. Apply the real 3.0.0 mixed define+undefine migration. This is the
    //    operation that fails under eager validation (SVL37).
    let migration = read_fixture("gist-3.0.0-migration.tql");
    apply_schema_migration(&target, database, &migration)
        .await
        .expect("gist 3.0.0 mixed-schema migration must apply under commit-time validation");

    server.stop();
}

fn read_fixture(relative: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()))
}
