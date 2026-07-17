//! Pure knowledge contracts shared by desktop adapters and future web services.
//!
//! Storage, transport and tenant adapters intentionally remain outside this crate.

/// Marker boundary until Phase 1A extracts retrieval implementation from desktop.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KnowledgeBoundary;
