//! FFI shim exposing [`vibe_ontology::apply`] to Swift via UniFFI.
//!
//! This is the boundary the OneApp Lingo plugin calls (through the generated
//! `OntologyCore` Swift module) so that applying schema/migrations goes through
//! the same Rust engine that validates them — one source of truth for the CLI,
//! CI, and the app (one-2xg.46). The domain logic lives entirely in
//! `vibe-ontology`; this crate only marshals types across the FFI boundary.

uniffi::setup_scaffolding!();

use vibe_ontology::apply::{self, TypeDbTarget};

/// Error surfaced across the FFI boundary. Flat by design — the Swift side only
/// needs a human-readable message (the underlying TypeDB/engine error string).
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum OntologyError {
    #[error("{message}")]
    Apply { message: String },
}

impl From<vibe_ontology::Error> for OntologyError {
    fn from(error: vibe_ontology::Error) -> Self {
        OntologyError::Apply {
            message: error.to_string(),
        }
    }
}

/// Connection details for a TypeDB server.
#[derive(Debug, Clone, uniffi::Record)]
pub struct TypeDbConnection {
    pub address: String,
    pub username: String,
    pub password: String,
    pub tls_enabled: bool,
}

impl From<TypeDbConnection> for TypeDbTarget {
    fn from(connection: TypeDbConnection) -> Self {
        TypeDbTarget {
            address: connection.address,
            username: connection.username,
            password: connection.password,
            tls_enabled: connection.tls_enabled,
        }
    }
}

/// Creates `database` on the target server (idempotent).
#[uniffi::export(async_runtime = "tokio")]
pub async fn create_database(
    connection: TypeDbConnection,
    database: String,
) -> Result<(), OntologyError> {
    apply::create_database(&connection.into(), &database).await?;
    Ok(())
}

/// Applies one or more schema TQL blobs as a single schema transaction with
/// commit-time validation.
#[uniffi::export(async_runtime = "tokio")]
pub async fn apply_schema(
    connection: TypeDbConnection,
    database: String,
    tql_blobs: Vec<String>,
) -> Result<(), OntologyError> {
    apply::apply_schema(&connection.into(), &database, &tql_blobs).await?;
    Ok(())
}

/// Applies a single migration TQL blob (convenience over [`apply_schema`]).
#[uniffi::export(async_runtime = "tokio")]
pub async fn apply_schema_migration(
    connection: TypeDbConnection,
    database: String,
    migration_tql: String,
) -> Result<(), OntologyError> {
    apply::apply_schema_migration(&connection.into(), &database, &migration_tql).await?;
    Ok(())
}
