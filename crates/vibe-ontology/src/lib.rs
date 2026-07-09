//! Vibe Machine ontology core library.
//!
//! Provides the durable, embeddable surface for executable ontology corpora:
//! parsing the corpus manifest, discovering corpus items, validating their
//! shape, and selecting items for prompt-ready export.
//!
//! It also owns **schema application** against a live TypeDB server (the
//! [`apply`] module) so that validating a migration and applying it share one
//! implementation — the single source of truth the CLI, CI, and the OneApp
//! Lingo plugin all call into.
//!
//! The crate has no CLI dependencies and is safe to embed inside other Vibe
//! Machine products (OneApp, Lingo, the `ont` CLI itself) without dragging clap
//! or ratatui into their build.

pub mod apply;
pub mod corpus;
pub mod error;
pub mod executable_package;
pub mod migration_contract;
pub mod package_validator;
pub mod version;

pub use error::{Error, Result};
