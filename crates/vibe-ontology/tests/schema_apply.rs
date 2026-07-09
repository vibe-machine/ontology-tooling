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

use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use vibe_ontology::apply::{apply_schema, apply_schema_migration, create_database, TypeDbTarget};

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

/// A locally-spawned `typedb server` bound to a free port, torn down on drop.
struct TypeDbServer {
    child: Option<Child>,
    address: String,
    _data_dir: tempfile::TempDir,
}

impl TypeDbServer {
    fn start() -> Self {
        let binary = locate_typedb();
        let port = free_port();
        let address = format!("127.0.0.1:{port}");
        let data_dir = tempfile::tempdir().expect("temp data dir");
        let logs_dir = data_dir.path().join("logs");

        let child = Command::new(&binary)
            .args([
                "server",
                "--server.address",
                &address,
                "--server.http.enabled",
                "false",
                "--server.admin.enabled",
                "false",
                "--diagnostics.monitoring.enabled",
                "false",
                "--storage.data-directory",
                data_dir.path().to_str().unwrap(),
                "--logging.directory",
                logs_dir.to_str().unwrap(),
            ])
            .spawn()
            .unwrap_or_else(|e| panic!("failed to launch {}: {e}", binary.display()));

        wait_for_listen(&address, Duration::from_secs(30));

        Self {
            child: Some(child),
            address,
            _data_dir: data_dir,
        }
    }

    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for TypeDbServer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn locate_typedb() -> PathBuf {
    if let Ok(bin) = std::env::var("TYPEDB_BIN") {
        let path = PathBuf::from(bin);
        assert!(
            path.exists(),
            "TYPEDB_BIN set but does not exist: {}",
            path.display()
        );
        return path;
    }
    for candidate in [
        "typedb",
        "/opt/homebrew/bin/typedb",
        "/usr/local/bin/typedb",
    ] {
        let path = PathBuf::from(candidate);
        if path.is_absolute() && path.exists() {
            return path;
        }
        if let Ok(found) = which(candidate) {
            return found;
        }
    }
    panic!(
        "no `typedb` binary found (set TYPEDB_BIN); this live apply test must not silently skip"
    );
}

fn which(name: &str) -> Result<PathBuf, ()> {
    let path_var = std::env::var_os("PATH").ok_or(())?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(())
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind free port")
        .local_addr()
        .expect("local addr")
        .port()
}

fn wait_for_listen(address: &str, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if TcpStream::connect(address).is_ok() {
            // Give the server a moment past the TCP accept to finish gRPC init.
            std::thread::sleep(Duration::from_millis(500));
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    panic!("typedb server did not become reachable at {address} within {timeout:?}");
}
