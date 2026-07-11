//! Persistent HNSW cache for the SQLite vector source of truth.
//! Corruption or signature mismatch is non-fatal: callers fall back to exact cosine.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use hnsw_rs::prelude::{DistCosine, Hnsw, HnswIo};
use serde::{Deserialize, Serialize};

const FORMAT_VERSION: u32 = 1;
const MIN_HNSW_POINTS: usize = 128;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    format_version: u32,
    signature: String,
    dimensions: usize,
    chunk_ids: Vec<String>,
    basename: String,
}

fn partition_name(signature: &str, dimensions: usize) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    signature.hash(&mut hasher);
    format!("{:016x}-{dimensions}", hasher.finish())
}

fn index_directory(root: &Path, signature: &str, dimensions: usize) -> PathBuf {
    root.join(".markhand/vector-index")
        .join(partition_name(signature, dimensions))
}

fn manifest_path(directory: &Path) -> PathBuf {
    directory.join("manifest.json")
}

pub fn clear(root: &Path) {
    let _ = std::fs::remove_dir_all(root.join(".markhand/vector-index"));
}

pub fn is_available(root: &Path, signature: &str, dimensions: usize) -> bool {
    manifest_path(&index_directory(root, signature, dimensions)).is_file()
}

pub fn rebuild(
    root: &Path,
    signature: &str,
    dimensions: usize,
    points: &[(String, Vec<f32>)],
) -> Result<bool, String> {
    let directory = index_directory(root, signature, dimensions);
    if points.len() < MIN_HNSW_POINTS {
        let _ = std::fs::remove_dir_all(directory);
        return Ok(false);
    }
    if dimensions == 0 || points.iter().any(|(_, vector)| vector.len() != dimensions) {
        return Err("không thể build HNSW với vector khác số chiều".into());
    }

    let parent = directory.parent().ok_or("HNSW directory không có parent")?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        partition_name(signature, dimensions),
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&temporary);
    std::fs::create_dir_all(&temporary).map_err(|error| error.to_string())?;

    let max_connections = 16;
    let max_layers = 16;
    let ef_construction = 200;
    let hnsw = Hnsw::<f32, DistCosine>::new(
        max_connections,
        points.len(),
        max_layers,
        ef_construction,
        DistCosine {},
    );
    for (index, (_, vector)) in points.iter().enumerate() {
        hnsw.insert((vector.as_slice(), index));
    }
    let basename = hnsw
        .file_dump(&temporary, "index")
        .map_err(|error| error.to_string())?;
    let manifest = Manifest {
        format_version: FORMAT_VERSION,
        signature: signature.into(),
        dimensions,
        chunk_ids: points.iter().map(|(id, _)| id.clone()).collect(),
        basename,
    };
    std::fs::write(
        manifest_path(&temporary),
        serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;

    let backup = parent.join(format!(
        ".{}.{}.old",
        partition_name(signature, dimensions),
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&backup);
    if directory.exists() {
        std::fs::rename(&directory, &backup).map_err(|error| error.to_string())?;
    }
    if let Err(error) = std::fs::rename(&temporary, &directory) {
        if backup.exists() {
            let _ = std::fs::rename(&backup, &directory);
        }
        return Err(error.to_string());
    }
    let _ = std::fs::remove_dir_all(backup);
    Ok(true)
}

pub fn search(
    root: &Path,
    signature: &str,
    dimensions: usize,
    query: &[f32],
    limit: usize,
) -> Result<Vec<(String, f32)>, String> {
    if query.len() != dimensions {
        return Err(format!(
            "query HNSW {}D không khớp index {dimensions}D",
            query.len()
        ));
    }
    let directory = index_directory(root, signature, dimensions);
    let manifest: Manifest = serde_json::from_slice(
        &std::fs::read(manifest_path(&directory)).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    if manifest.format_version != FORMAT_VERSION
        || manifest.signature != signature
        || manifest.dimensions != dimensions
    {
        return Err("HNSW manifest không khớp SQLite metadata".into());
    }
    let mut loader = HnswIo::new(&directory, &manifest.basename);
    let hnsw: Hnsw<f32, DistCosine> = loader.load_hnsw().map_err(|error| error.to_string())?;
    if hnsw.get_nb_point() != manifest.chunk_ids.len() {
        return Err("HNSW point count không khớp manifest".into());
    }
    let neighbours = hnsw.search(query, limit.min(manifest.chunk_ids.len()), 96);
    Ok(neighbours
        .into_iter()
        .filter_map(|neighbour| {
            manifest
                .chunk_ids
                .get(neighbour.d_id)
                .map(|id| (id.clone(), (1.0 - neighbour.distance).clamp(-1.0, 1.0)))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistent_hnsw_round_trip_finds_identical_vector() {
        let root = std::env::temp_dir().join(format!("markhand_hnsw_{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let points: Vec<(String, Vec<f32>)> = (0..256)
            .map(|index| {
                let mut vector = vec![0.0; 16];
                vector[index % 16] = 1.0;
                vector[(index * 7 + 3) % 16] += index as f32 / 512.0;
                let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
                for value in &mut vector {
                    *value /= norm;
                }
                (format!("chunk-{index}"), vector)
            })
            .collect();
        assert!(rebuild(&root, "test-signature", 16, &points).unwrap());
        let result = search(&root, "test-signature", 16, &points[173].1, 10).unwrap();
        assert_eq!(result[0].0, "chunk-173");
        clear(&root);
        let _ = std::fs::remove_dir_all(root);
    }
}
