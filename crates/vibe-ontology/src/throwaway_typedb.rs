//! A throwaway local TypeDB server: started on a free port with its data and
//! logs in a fresh temporary directory, stopped and removed on drop (including
//! on failure).
//!
//! Mirrors the live-test harness (`tests/common/mod.rs`) and the package
//! bootstrap test (`tools/package_contract/validate_typedb_bootstrap.mjs`):
//! HTTP, admin and monitoring are disabled so it never collides with a
//! developer's running server.

use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::apply::TypeDbTarget;
use crate::error::{Error, Result};

/// A running throwaway server. Dropping it kills the process and removes its
/// temporary directory.
pub struct ThrowawayTypeDb {
    child: Option<Child>,
    temp_dir: PathBuf,
    pub address: String,
}

impl ThrowawayTypeDb {
    /// Starts a server from `binary` (or [`locate_typedb`] when `None`) and
    /// waits until it accepts connections.
    pub fn start(binary: Option<&Path>) -> Result<Self> {
        let binary = match binary {
            Some(binary) => binary.to_path_buf(),
            None => locate_typedb()?,
        };
        let port = free_port()?;
        let address = format!("127.0.0.1:{port}");
        let temp_dir = create_temp_dir()?;
        let data_dir = temp_dir.join("data");
        let logs_dir = temp_dir.join("logs");

        let spawned = Command::new(&binary)
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
            ])
            .arg("--storage.data-directory")
            .arg(&data_dir)
            .arg("--logging.directory")
            .arg(&logs_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let child = match spawned {
            Ok(child) => child,
            Err(error) => {
                let _ = std::fs::remove_dir_all(&temp_dir);
                return Err(Error::TypeDb(format!(
                    "failed to launch {}: {error}",
                    binary.display()
                )));
            }
        };

        // From here on, drop stops the server and removes the directory.
        let mut server = Self {
            child: Some(child),
            temp_dir,
            address,
        };
        server.wait_until_listening(Duration::from_secs(60))?;
        Ok(server)
    }

    /// Connection details for the server (admin/password, no TLS).
    pub fn target(&self) -> TypeDbTarget {
        TypeDbTarget::local(self.address.clone())
    }

    fn wait_until_listening(&mut self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Some(child) = self.child.as_mut() {
                if let Ok(Some(status)) = child.try_wait() {
                    return Err(Error::TypeDb(format!(
                        "typedb server exited early ({status}); logs in {}",
                        self.temp_dir.join("logs").display()
                    )));
                }
            }
            if TcpStream::connect(&self.address).is_ok() {
                // Give the server a moment past the TCP accept to finish gRPC init.
                std::thread::sleep(Duration::from_millis(500));
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        Err(Error::TypeDb(format!(
            "typedb server did not become reachable at {} within {timeout:?}",
            self.address
        )))
    }

    /// Stops the server and removes its temporary directory.
    pub fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.temp_dir);
    }
}

impl Drop for ThrowawayTypeDb {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Finds a `typedb` binary: `TYPEDB_BIN`, then `typedb` on `PATH`, then
/// `~/.typedb/typedb`, then the Homebrew locations.
pub fn locate_typedb() -> Result<PathBuf> {
    if let Some(bin) = std::env::var_os("TYPEDB_BIN") {
        let path = PathBuf::from(bin);
        if path.is_file() {
            return Ok(path);
        }
        return Err(Error::TypeDb(format!(
            "TYPEDB_BIN is set but does not exist: {}",
            path.display()
        )));
    }
    let mut candidates: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path)
                .map(|directory| directory.join("typedb"))
                .collect()
        })
        .unwrap_or_default();
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(PathBuf::from(home).join(".typedb").join("typedb"));
    }
    candidates.push(PathBuf::from("/opt/homebrew/bin/typedb"));
    candidates.push(PathBuf::from("/usr/local/bin/typedb"));
    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| {
            Error::TypeDb(
                "TypeDB executable not found. Install it on PATH or set TYPEDB_BIN.".to_string(),
            )
        })
}

fn free_port() -> Result<u16> {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .map_err(|error| Error::TypeDb(format!("could not find a free port: {error}")))
}

fn create_temp_dir() -> Result<PathBuf> {
    let base = std::env::temp_dir();
    for attempt in 0..100u32 {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let path = base.join(format!(
            "ont-typedb-{}-{nanos}-{attempt}",
            std::process::id()
        ));
        match std::fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(source) => return Err(Error::Io { path, source }),
        }
    }
    Err(Error::TypeDb(format!(
        "could not create a temporary directory under {}",
        base.display()
    )))
}
