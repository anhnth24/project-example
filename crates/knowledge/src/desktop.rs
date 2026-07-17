//! Optional desktop adapter dependency markers.
//!
//! Concrete SQLite/HNSW implementation moves here in later extraction issues.

#[cfg(feature = "desktop-sqlite")]
pub fn sqlite_adapter_enabled() -> bool {
    !rusqlite::version().is_empty()
}

#[cfg(feature = "desktop-hnsw")]
pub fn hnsw_adapter_enabled() -> bool {
    !std::any::type_name::<hnsw_rs::prelude::DistCosine>().is_empty()
}
