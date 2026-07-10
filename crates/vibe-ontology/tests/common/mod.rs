// Shared across multiple integration-test crates; not every test uses every
// helper, so suppress per-crate dead-code warnings for the unused ones.
#![allow(dead_code)]

//! Shared harness for the live TypeDB integration tests: spins up a local
//! `typedb server` on a free port with admin/http/monitoring disabled (so it
//! never collides with a developer's already-running TypeDB), torn down on drop.
//!
//! Requires a `typedb` binary (env `TYPEDB_BIN`, else `typedb` on PATH, else the
//! Homebrew location). If none is found the harness panics rather than letting a
//! live test silently skip — a live apply test that skips is worthless.

use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

/// A locally-spawned `typedb server` bound to a free port, torn down on drop.
pub struct TypeDbServer {
    child: Option<Child>,
    pub address: String,
    _data_dir: tempfile::TempDir,
}

impl TypeDbServer {
    pub fn start() -> Self {
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

    /// Returns true if the server process is still running (used to assert a
    /// bootstrap did not crash the server).
    pub fn is_alive(&mut self) -> bool {
        match self.child.as_mut() {
            Some(child) => matches!(child.try_wait(), Ok(None)),
            None => false,
        }
    }

    pub fn stop(&mut self) {
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
