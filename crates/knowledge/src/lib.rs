//! Pure knowledge contracts shared by desktop adapters and future web services.
//!
//! Storage and transport remain outside the default build. Desktop-only SQLite/HNSW
//! adapters are isolated behind explicit features.

pub mod ask;
pub mod citation;
pub mod embedding;
pub mod error;
pub mod identity;
pub mod query;
pub mod rank;
pub mod types;

#[cfg(any(feature = "desktop-hnsw", feature = "desktop-sqlite"))]
pub mod desktop;

pub use error::{KnowledgeError, Result};
