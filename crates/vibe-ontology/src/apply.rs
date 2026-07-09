//! Schema application against a live TypeDB server.
//!
//! This is the apply half of the ontology engine: the single place that owns
//! *submitting* schema and migrations to TypeDB. It exists because the app's
//! Swift/C-driver apply path validates each schema query **eagerly** (it
//! resolves every query's promise as it is submitted), which rejects otherwise
//! valid migrations whose intermediate state is transiently invalid — e.g. the
//! gist 2.0.4 → 3.0.0 structural-property-hierarchy migration, which first
//! subtypes an attribute (so it *inherits* a value type) and then undefines the
//! now-redundant direct value declaration. Under eager validation the undefine
//! fails (`SVL37: cannot unset an inherited value type`); under **commit-time**
//! validation the net final state is valid and the transaction commits.
//!
//! TypeDB console applies that migration cleanly because it submits each query
//! in one schema transaction and lets the server validate at commit. This
//! module reproduces exactly that behavior via the official TypeDB Rust driver
//! (the same driver console is built on), so validation and application share
//! one implementation. See one-2i7 / one-2xg.46.

use crate::error::{Error, Result};
use typedb_driver::{
    Addresses, Credentials, DriverOptions, DriverTlsConfig, TransactionType, TypeDBDriver,
};

/// Connection target for a TypeDB server.
#[derive(Debug, Clone)]
pub struct TypeDbTarget {
    pub address: String,
    pub username: String,
    pub password: String,
    pub tls_enabled: bool,
}

impl TypeDbTarget {
    /// A local, TLS-disabled server with the default `admin`/`password`
    /// credentials — the shape used by tests and the bundled dev server.
    pub fn local(address: impl Into<String>) -> Self {
        Self {
            address: address.into(),
            username: "admin".to_string(),
            password: "password".to_string(),
            tls_enabled: false,
        }
    }
}

/// Splits a schema TQL blob into its individual top-level operations
/// (`define` / `undefine` / `redefine`), preserving order.
///
/// A migration file is authored as consecutive top-level blocks (a `define`
/// block, then an `undefine` block); TypeDB accepts each as its own query but
/// rejects the two concatenated into a single query (`TQL0` parser error).
/// Splitting here — then submitting the pieces within one transaction — is what
/// lets commit-time validation see only the valid final state. Mirrors TypeDB
/// console's `source` behavior.
pub fn split_schema_queries(tql: &str) -> Vec<String> {
    fn is_operation_keyword(line: &str) -> bool {
        matches!(line.trim(), "define" | "undefine" | "redefine")
    }

    let mut chunks: Vec<String> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    // Only break at a keyword once a query is already open, so leading (and
    // interstitial) comment lines attach to the following operation instead of
    // being flushed as a bare comment "query" (which is a TQL0 parse error).
    let mut current_has_operation = false;

    for line in tql.lines() {
        if is_operation_keyword(line) {
            if current_has_operation {
                push_chunk(&mut chunks, &current);
                current.clear();
            }
            current_has_operation = true;
        }
        current.push(line);
    }
    push_chunk(&mut chunks, &current);
    chunks
}

fn push_chunk(chunks: &mut Vec<String>, lines: &[&str]) {
    let chunk = lines.join("\n").trim().to_string();
    if !chunk.is_empty() {
        chunks.push(chunk);
    }
}

/// Opens a connection to the target server.
async fn connect(target: &TypeDbTarget) -> Result<TypeDBDriver> {
    let addresses = Addresses::try_from_address_str(&target.address).map_err(typedb_err)?;
    let credentials = Credentials::new(&target.username, &target.password);
    let tls = if target.tls_enabled {
        DriverTlsConfig::enabled_with_native_root_ca()
    } else {
        DriverTlsConfig::disabled()
    };
    TypeDBDriver::new(addresses, credentials, DriverOptions::new(tls))
        .await
        .map_err(typedb_err)
}

/// Creates `database` on the target server (idempotent: no-op if it exists).
pub async fn create_database(target: &TypeDbTarget, database: &str) -> Result<()> {
    let driver = connect(target).await?;
    if driver
        .databases()
        .contains(database)
        .await
        .map_err(typedb_err)?
    {
        return Ok(());
    }
    driver
        .databases()
        .create(database)
        .await
        .map_err(typedb_err)?;
    Ok(())
}

/// Applies one or more schema TQL blobs to `database` as a **single schema
/// transaction with commit-time validation**. Each blob is split into its
/// top-level operations and submitted in order; the server validates the whole
/// transaction at commit. This is the apply semantics the eager C-driver path
/// lacks.
pub async fn apply_schema<S: AsRef<str>>(
    target: &TypeDbTarget,
    database: &str,
    tql_blobs: &[S],
) -> Result<()> {
    let driver = connect(target).await?;
    let transaction = driver
        .transaction(database, TransactionType::Schema)
        .await
        .map_err(typedb_err)?;

    for blob in tql_blobs {
        for query in split_schema_queries(blob.as_ref()) {
            transaction.query(query).await.map_err(typedb_err)?;
        }
    }

    transaction.commit().await.map_err(typedb_err)?;
    Ok(())
}

/// Applies a single migration TQL blob (convenience over [`apply_schema`]).
pub async fn apply_schema_migration(
    target: &TypeDbTarget,
    database: &str,
    migration_tql: &str,
) -> Result<()> {
    apply_schema(target, database, &[migration_tql]).await
}

fn typedb_err<E: std::fmt::Display>(error: E) -> Error {
    Error::TypeDb(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::split_schema_queries;

    #[test]
    fn leading_comments_attach_to_the_following_operation() {
        // Regression: a comment block before `define` must not be emitted as a
        // bare comment "query" (TQL0 parse error).
        let tql = "# header comment\n# more\ndefine\nattribute a value string;";
        let chunks = split_schema_queries(tql);
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].starts_with("# header comment"));
        assert!(chunks[0].contains("define"));
    }

    #[test]
    fn splits_define_then_undefine_into_two_operations() {
        let tql = "define\nattribute a sub b;\n\nundefine\nvalue string from a;";
        let chunks = split_schema_queries(tql);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].starts_with("define"));
        assert!(chunks[1].starts_with("undefine"));
    }

    #[test]
    fn interstitial_comment_attaches_to_preceding_operation() {
        // A comment between the define block and the undefine keyword trails the
        // define query (inert); the split still yields exactly two operations.
        let tql = "define\nattribute a sub b;\n# note\nundefine\nvalue string from a;";
        let chunks = split_schema_queries(tql);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].contains("# note"));
        assert!(chunks[1].starts_with("undefine"));
    }

    #[test]
    fn single_define_is_one_chunk() {
        let chunks = split_schema_queries("define\nattribute a value string;");
        assert_eq!(chunks.len(), 1);
    }

    #[test]
    fn empty_input_yields_no_chunks() {
        assert!(split_schema_queries("   \n\n").is_empty());
    }
}
