//! Desktop compatibility adapter for the shared persistent HNSW cache.

use std::path::Path;

use fileconv_knowledge::desktop::hnsw;

pub fn clear(root: &Path) {
    let _ = hnsw::clear(root);
}

pub fn is_available(root: &Path, signature: &str, dimensions: usize) -> bool {
    hnsw::is_available(root, signature, dimensions)
}

pub fn rebuild(
    root: &Path,
    signature: &str,
    dimensions: usize,
    points: &[(String, Vec<f32>)],
) -> Result<bool, String> {
    hnsw::rebuild(root, signature, dimensions, points).map_err(|error| error.to_string())
}

pub fn search(
    root: &Path,
    signature: &str,
    dimensions: usize,
    query: &[f32],
    limit: usize,
) -> Result<Vec<(String, f32)>, String> {
    hnsw::search(root, signature, dimensions, query, limit).map_err(|error| error.to_string())
}
