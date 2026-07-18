//! Persistent HNSW cache for the SQLite vector source of truth.
//!
//! Corruption or signature mismatch is non-fatal to callers: SQLite remains
//! authoritative and desktop search falls back to exact cosine.

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use hnsw_rs::api::AnnT;
use hnsw_rs::prelude::{DistCosine, Hnsw, HnswIo};
use serde::{Deserialize, Serialize};
use siphasher::sip::SipHasher13;

use crate::{KnowledgeError, Result};

pub const FORMAT_VERSION: u32 = 1;
pub const MIN_HNSW_POINTS: usize = 128;
pub const MAX_HNSW_POINTS: usize = 100_000;
const MAX_VECTOR_DIMENSIONS: usize = 4_096;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    format_version: u32,
    signature: String,
    dimensions: usize,
    chunk_ids: Vec<String>,
    basename: String,
}

fn failure(message: impl Into<String>) -> KnowledgeError {
    KnowledgeError::AdapterFailure(message.into())
}

fn partition_name(signature: &str, dimensions: usize) -> String {
    // Explicit SipHash-1-3 preserves the legacy DefaultHasher partition path.
    let mut hasher = SipHasher13::new();
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

fn validate_dimensions(dimensions: usize) -> Result<()> {
    if !(1..=MAX_VECTOR_DIMENSIONS).contains(&dimensions) {
        return Err(KnowledgeError::InvalidInput(
            "HNSW dimensions must be 1..=4096",
        ));
    }
    Ok(())
}

fn validate_manifest(manifest: &Manifest, signature: &str, dimensions: usize) -> Result<()> {
    if manifest.format_version != FORMAT_VERSION
        || manifest.signature != signature
        || manifest.dimensions != dimensions
    {
        return Err(KnowledgeError::IncompatibleIndex(
            "HNSW manifest does not match SQLite metadata",
        ));
    }
    if !(MIN_HNSW_POINTS..=MAX_HNSW_POINTS).contains(&manifest.chunk_ids.len()) {
        return Err(KnowledgeError::IncompatibleIndex(
            "HNSW manifest point count is out of bounds",
        ));
    }
    if manifest.chunk_ids.iter().any(|id| id.is_empty())
        || manifest.chunk_ids.iter().collect::<HashSet<_>>().len() != manifest.chunk_ids.len()
    {
        return Err(KnowledgeError::IncompatibleIndex(
            "HNSW manifest contains invalid chunk identifiers",
        ));
    }
    if manifest.basename.is_empty()
        || manifest
            .basename
            .chars()
            .any(|character| !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_')))
    {
        return Err(KnowledgeError::IncompatibleIndex(
            "HNSW manifest basename is unsafe",
        ));
    }
    Ok(())
}

fn read_manifest(directory: &Path, signature: &str, dimensions: usize) -> Result<Manifest> {
    validate_dimensions(dimensions)?;
    let bytes = std::fs::read(manifest_path(directory))
        .map_err(|error| failure(format!("cannot read HNSW manifest: {error}")))?;
    let manifest: Manifest = serde_json::from_slice(&bytes)
        .map_err(|_| KnowledgeError::IncompatibleIndex("HNSW manifest is corrupt"))?;
    validate_manifest(&manifest, signature, dimensions)?;
    Ok(manifest)
}

pub fn clear(root: &Path) -> Result<()> {
    let path = root.join(".markhand/vector-index");
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(failure(format!("cannot clear HNSW cache: {error}"))),
    }
}

pub fn is_available(root: &Path, signature: &str, dimensions: usize) -> bool {
    let directory = index_directory(root, signature, dimensions);
    read_manifest(&directory, signature, dimensions).is_ok()
}

pub fn rebuild(
    root: &Path,
    signature: &str,
    dimensions: usize,
    points: &[(String, Vec<f32>)],
) -> Result<bool> {
    validate_dimensions(dimensions)?;
    let directory = index_directory(root, signature, dimensions);
    if points.len() < MIN_HNSW_POINTS {
        match std::fs::remove_dir_all(directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(failure(format!("cannot remove stale HNSW cache: {error}"))),
        }
        return Ok(false);
    }
    if points.len() > MAX_HNSW_POINTS {
        return Err(KnowledgeError::InvalidInput(
            "HNSW point count exceeds 100000",
        ));
    }
    if points
        .iter()
        .any(|(id, vector)| id.is_empty() || vector.len() != dimensions)
    {
        return Err(KnowledgeError::InvalidInput(
            "HNSW point identifier or dimensions are invalid",
        ));
    }
    if points
        .iter()
        .map(|(id, _)| id)
        .collect::<HashSet<_>>()
        .len()
        != points.len()
    {
        return Err(KnowledgeError::InvalidInput(
            "HNSW point identifiers must be unique",
        ));
    }

    let parent = directory
        .parent()
        .ok_or(KnowledgeError::AdapterUnavailable(
            "HNSW directory has no parent",
        ))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| failure(format!("cannot create HNSW parent: {error}")))?;
    let partition = partition_name(signature, dimensions);
    let temporary = parent.join(format!(".{partition}.{}.tmp", std::process::id()));
    match std::fs::remove_dir_all(&temporary) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(failure(format!(
                "cannot clear temporary HNSW cache: {error}"
            )))
        }
    }
    std::fs::create_dir_all(&temporary)
        .map_err(|error| failure(format!("cannot create temporary HNSW cache: {error}")))?;

    let hnsw = Hnsw::<f32, DistCosine>::new(16, points.len(), 16, 200, DistCosine {});
    for (index, (_, vector)) in points.iter().enumerate() {
        hnsw.insert((vector.as_slice(), index));
    }
    let basename = hnsw
        .file_dump(&temporary, "index")
        .map_err(|error| failure(format!("cannot dump HNSW cache: {error}")))?;
    let manifest = Manifest {
        format_version: FORMAT_VERSION,
        signature: signature.into(),
        dimensions,
        chunk_ids: points.iter().map(|(id, _)| id.clone()).collect(),
        basename,
    };
    validate_manifest(&manifest, signature, dimensions)?;
    std::fs::write(
        manifest_path(&temporary),
        serde_json::to_vec_pretty(&manifest)
            .map_err(|error| failure(format!("cannot encode HNSW manifest: {error}")))?,
    )
    .map_err(|error| failure(format!("cannot write HNSW manifest: {error}")))?;

    let backup = parent.join(format!(".{partition}.{}.old", std::process::id()));
    match std::fs::remove_dir_all(&backup) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(failure(format!("cannot clear HNSW backup: {error}"))),
    }
    if directory.exists() {
        std::fs::rename(&directory, &backup)
            .map_err(|error| failure(format!("cannot stage old HNSW cache: {error}")))?;
    }
    if let Err(error) = std::fs::rename(&temporary, &directory) {
        if backup.exists() {
            let _ = std::fs::rename(&backup, &directory);
        }
        return Err(failure(format!("cannot activate HNSW cache: {error}")));
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
) -> Result<Vec<(String, f32)>> {
    validate_dimensions(dimensions)?;
    if query.len() != dimensions {
        return Err(KnowledgeError::EmbeddingDimensionMismatch {
            expected: dimensions,
            actual: query.len(),
        });
    }
    if limit == 0 {
        return Ok(Vec::new());
    }
    let directory = index_directory(root, signature, dimensions);
    let manifest = read_manifest(&directory, signature, dimensions)?;
    let mut loader = HnswIo::new(&directory, &manifest.basename);
    let hnsw: Hnsw<f32, DistCosine> = loader
        .load_hnsw()
        .map_err(|error| failure(format!("cannot load HNSW cache: {error}")))?;
    if hnsw.get_nb_point() != manifest.chunk_ids.len() {
        return Err(KnowledgeError::IncompatibleIndex(
            "HNSW point count does not match manifest",
        ));
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

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "markhand_hnsw_{label}_{}_{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn points() -> Vec<(String, Vec<f32>)> {
        (0..256)
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
            .collect()
    }

    #[test]
    fn persistent_round_trip_finds_identical_vector() {
        let root = temp_root("round_trip");
        let points = points();
        assert!(rebuild(&root, "test-signature", 16, &points).unwrap());
        assert!(is_available(&root, "test-signature", 16));
        let result = search(&root, "test-signature", 16, &points[173].1, 10).unwrap();
        assert_eq!(result[0].0, "chunk-173");
        clear(&root).unwrap();
        assert!(!is_available(&root, "test-signature", 16));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn too_few_points_remove_stale_partition() {
        let root = temp_root("small");
        let points = points();
        rebuild(&root, "test-signature", 16, &points).unwrap();
        assert!(!rebuild(&root, "test-signature", 16, &points[..2]).unwrap());
        assert!(!is_available(&root, "test-signature", 16));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn corrupt_mismatch_and_count_do_not_become_available() {
        let root = temp_root("corrupt");
        let directory = index_directory(&root, "test-signature", 16);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(manifest_path(&directory), b"not-json").unwrap();
        assert!(!is_available(&root, "test-signature", 16));

        let points = points();
        rebuild(&root, "test-signature", 16, &points).unwrap();
        assert!(!is_available(&root, "other-signature", 16));
        let mut manifest = read_manifest(&directory, "test-signature", 16).unwrap();
        manifest.chunk_ids.pop();
        std::fs::write(
            manifest_path(&directory),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert!(search(&root, "test-signature", 16, &points[0].1, 1).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_partition_hash_is_stable() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../fixtures/legacy-hnsw-v1.json")).unwrap();
        let signature = fixture["signature"].as_str().unwrap();
        let dimensions = fixture["dimensions"].as_u64().unwrap() as usize;
        assert_eq!(
            partition_name(signature, dimensions),
            fixture["partition"].as_str().unwrap()
        );
    }
}
