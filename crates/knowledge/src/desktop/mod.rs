//! Optional local desktop storage and ANN adapters.

#[cfg(feature = "desktop-hnsw")]
pub mod hnsw;

#[cfg(feature = "desktop-sqlite")]
pub mod sqlite;

#[cfg(all(feature = "desktop-hnsw", feature = "desktop-sqlite"))]
pub mod service;

#[cfg(feature = "desktop-hnsw")]
pub fn hnsw_adapter_enabled() -> bool {
    !std::any::type_name::<hnsw_rs::prelude::DistCosine>().is_empty()
}
