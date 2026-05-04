//! Vibe Machine ontology core library.
//!
//! Provides the durable, embeddable surface for executable ontology corpora:
//! parsing the corpus manifest, discovering corpus items, validating their
//! shape, and selecting items for prompt-ready export.
//!
//! This crate has no I/O beyond the filesystem, no CLI dependencies, and is
//! safe to embed inside other Vibe Machine products (OneApp, Lingo, the `ont`
//! CLI itself) without dragging clap or ratatui into their build.

pub mod corpus;
pub mod error;

pub use error::{Error, Result};
