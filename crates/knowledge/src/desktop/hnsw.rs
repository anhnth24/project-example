//! Persistent HNSW cache for the SQLite vector source of truth.
//!
//! Corruption, embedding-signature mismatch, or intelligence ID scheme mismatch
//! is non-fatal to callers: SQLite remains authoritative and desktop search
//! falls back to exact cosine. SQLite and HNSW are **not** one atomic
//! transaction — manifests carry `idScheme` so a stale ANN partition can never
//! stay usable after SQLite moves to a new durable ID scheme.

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::hash::{Hash, Hasher};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use fs2::FileExt;
use hnsw_rs::api::AnnT;
use hnsw_rs::prelude::{DistCosine, Hnsw, HnswIo};
use serde::{Deserialize, Serialize};
use siphasher::sip::SipHasher13;

use crate::{KnowledgeError, Result};

pub const FORMAT_VERSION: u32 = 1;
pub const MIN_HNSW_POINTS: usize = 128;
pub const MAX_HNSW_POINTS: usize = 100_000;
const MAX_VECTOR_DIMENSIONS: usize = 4_096;
const MAX_MANIFEST_BYTES: u64 = 32 * 1024 * 1024;
const MAX_SIGNATURE_BYTES: usize = 1_024;
const MAX_CHUNK_ID_BYTES: usize = 4_096;
const MAX_BASENAME_BYTES: usize = 128;
static TEMP_NONCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    format_version: u32,
    signature: String,
    /// Intelligence durable ID scheme (`sha256-v1`). Missing/empty = legacy.
    #[serde(default)]
    id_scheme: String,
    dimensions: usize,
    chunk_ids: Vec<String>,
    basename: String,
}

fn failure(message: impl Into<String>) -> KnowledgeError {
    KnowledgeError::AdapterFailure(message.into())
}

fn lock_file(root: &Path) -> Result<File> {
    let markhand = root.join(".markhand");
    std::fs::create_dir_all(&markhand)
        .map_err(|error| failure(format!("cannot create HNSW lock directory: {error}")))?;
    OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(markhand.join("vector-index.lock"))
        .map_err(|error| failure(format!("cannot open HNSW cache lock: {error}")))
}

fn with_cache_lock<T>(
    root: &Path,
    exclusive: bool,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let file = lock_file(root)?;
    if exclusive {
        FileExt::lock_exclusive(&file)
    } else {
        FileExt::lock_shared(&file)
    }
    .map_err(|error| failure(format!("cannot lock HNSW cache: {error}")))?;
    if exclusive {
        if let Err(error) = recover_interrupted_replacement(root) {
            let _ = FileExt::unlock(&file);
            return Err(error);
        }
    }
    let result = operation();
    let unlock = FileExt::unlock(&file)
        .map_err(|error| failure(format!("cannot unlock HNSW cache: {error}")));
    match (result, unlock) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
    }
}

fn recover_interrupted_replacement(root: &Path) -> Result<()> {
    let parent = root.join(".markhand/vector-index");
    let entries = match std::fs::read_dir(&parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(failure(format!(
                "cannot inspect HNSW recovery state: {error}"
            )))
        }
    };
    let mut paths = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with('.') && name.ends_with(".tmp") {
            std::fs::remove_dir_all(&path)
                .map_err(|error| failure(format!("cannot remove stale HNSW staging: {error}")))?;
            continue;
        }
        let Some(rest) = name
            .strip_prefix('.')
            .and_then(|name| name.strip_suffix(".old"))
        else {
            continue;
        };
        let Some(partition) = rest.split('.').next() else {
            continue;
        };
        if !valid_partition_name(partition) {
            continue;
        }
        let active = parent.join(partition);
        if active.exists() {
            std::fs::remove_dir_all(&path)
                .map_err(|error| failure(format!("cannot remove stale HNSW backup: {error}")))?;
        } else {
            std::fs::rename(&path, &active)
                .map_err(|error| failure(format!("cannot recover HNSW backup: {error}")))?;
        }
    }
    Ok(())
}

fn valid_partition_name(value: &str) -> bool {
    let Some((hash, dimensions)) = value.split_once('-') else {
        return false;
    };
    hash.len() == 16
        && hash.chars().all(|character| character.is_ascii_hexdigit())
        && !dimensions.is_empty()
        && dimensions
            .chars()
            .all(|character| character.is_ascii_digit())
}

fn partition_name(signature: &str, id_scheme: &str, dimensions: usize) -> String {
    // Explicit SipHash-1-3: empty id_scheme preserves the legacy partition path
    // (signature-only). Non-empty schemes domain-separate generations so a
    // stale ANN directory cannot be addressed under a new SQLite ID scheme.
    let mut hasher = SipHasher13::new();
    signature.hash(&mut hasher);
    if !id_scheme.is_empty() {
        0xff_u8.hash(&mut hasher);
        id_scheme.hash(&mut hasher);
    }
    format!("{:016x}-{dimensions}", hasher.finish())
}

fn index_directory(root: &Path, signature: &str, id_scheme: &str, dimensions: usize) -> PathBuf {
    root.join(".markhand/vector-index")
        .join(partition_name(signature, id_scheme, dimensions))
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

fn validate_manifest(
    manifest: &Manifest,
    signature: &str,
    id_scheme: &str,
    dimensions: usize,
) -> Result<()> {
    if manifest.format_version != FORMAT_VERSION
        || manifest.signature != signature
        || manifest.id_scheme != id_scheme
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
    if manifest.signature.len() > MAX_SIGNATURE_BYTES
        || manifest
            .chunk_ids
            .iter()
            .any(|id| id.is_empty() || id.len() > MAX_CHUNK_ID_BYTES)
        || manifest.chunk_ids.iter().collect::<HashSet<_>>().len() != manifest.chunk_ids.len()
    {
        return Err(KnowledgeError::IncompatibleIndex(
            "HNSW manifest contains invalid chunk identifiers",
        ));
    }
    if manifest.basename.is_empty()
        || manifest.basename.len() > MAX_BASENAME_BYTES
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

fn read_manifest(
    directory: &Path,
    signature: &str,
    id_scheme: &str,
    dimensions: usize,
) -> Result<Manifest> {
    validate_dimensions(dimensions)?;
    let path = manifest_path(directory);
    let metadata = std::fs::metadata(&path)
        .map_err(|error| failure(format!("cannot inspect HNSW manifest: {error}")))?;
    if metadata.len() > MAX_MANIFEST_BYTES {
        return Err(KnowledgeError::IncompatibleIndex(
            "HNSW manifest exceeds size limit",
        ));
    }
    let bytes = std::fs::read(path)
        .map_err(|error| failure(format!("cannot read HNSW manifest: {error}")))?;
    let manifest: Manifest = serde_json::from_slice(&bytes)
        .map_err(|_| KnowledgeError::IncompatibleIndex("HNSW manifest is corrupt"))?;
    validate_manifest(&manifest, signature, id_scheme, dimensions)?;
    Ok(manifest)
}

fn validate_index_files(directory: &Path, manifest: &Manifest, dimensions: usize) -> Result<()> {
    catch_unwind(AssertUnwindSafe(|| {
        let mut loader = HnswIo::new(directory, &manifest.basename);
        let hnsw: Hnsw<f32, DistCosine> = loader
            .load_hnsw()
            .map_err(|error| failure(format!("cannot load HNSW cache: {error}")))?;
        if hnsw.get_nb_point() != manifest.chunk_ids.len() {
            return Err(KnowledgeError::IncompatibleIndex(
                "HNSW point count does not match manifest",
            ));
        }
        if hnsw.get_point_indexation().get_data_dimension() != dimensions {
            return Err(KnowledgeError::IncompatibleIndex(
                "HNSW data dimensions do not match manifest",
            ));
        }
        Ok(())
    }))
    .map_err(|_| KnowledgeError::IncompatibleIndex("HNSW cache panicked while loading"))?
}

pub fn clear(root: &Path) -> Result<()> {
    with_cache_lock(root, true, || {
        let path = root.join(".markhand/vector-index");
        match std::fs::remove_dir_all(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(failure(format!("cannot clear HNSW cache: {error}"))),
        }
    })
}

pub fn is_available(root: &Path, signature: &str, id_scheme: &str, dimensions: usize) -> bool {
    with_cache_lock(root, true, || {
        let directory = index_directory(root, signature, id_scheme, dimensions);
        let manifest = read_manifest(&directory, signature, id_scheme, dimensions)?;
        validate_index_files(&directory, &manifest, dimensions)
    })
    .is_ok()
}

pub fn rebuild(
    root: &Path,
    signature: &str,
    id_scheme: &str,
    dimensions: usize,
    points: &[(String, Vec<f32>)],
) -> Result<bool> {
    with_cache_lock(root, true, || {
        rebuild_locked(root, signature, id_scheme, dimensions, points)
    })
}

fn rebuild_locked(
    root: &Path,
    signature: &str,
    id_scheme: &str,
    dimensions: usize,
    points: &[(String, Vec<f32>)],
) -> Result<bool> {
    validate_dimensions(dimensions)?;
    let directory = index_directory(root, signature, id_scheme, dimensions);
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
    if points.iter().any(|(id, vector)| {
        id.is_empty() || vector.len() != dimensions || vector.iter().any(|value| !value.is_finite())
    }) {
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
    let worst_case_manifest_bytes = points
        .iter()
        .try_fold(
            signature.len().saturating_mul(6) + 1_024,
            |total, (id, _)| total.checked_add(id.len().saturating_mul(6) + 8),
        )
        .ok_or(KnowledgeError::InvalidInput("HNSW manifest size overflow"))?;
    if worst_case_manifest_bytes > MAX_MANIFEST_BYTES as usize {
        return Err(KnowledgeError::InvalidInput(
            "HNSW manifest would exceed size limit",
        ));
    }

    let parent = directory
        .parent()
        .ok_or(KnowledgeError::AdapterUnavailable(
            "HNSW directory has no parent",
        ))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| failure(format!("cannot create HNSW parent: {error}")))?;
    let partition = partition_name(signature, id_scheme, dimensions);
    let nonce = TEMP_NONCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(".{partition}.{}.{nonce}.tmp", std::process::id()));
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

    let build = catch_unwind(AssertUnwindSafe(|| {
        let hnsw = Hnsw::<f32, DistCosine>::new(16, points.len(), 16, 200, DistCosine {});
        for (index, (_, vector)) in points.iter().enumerate() {
            hnsw.insert((vector.as_slice(), index));
        }
        hnsw.file_dump(&temporary, "index")
            .map_err(|error| failure(format!("cannot dump HNSW cache: {error}")))
    }))
    .map_err(|_| KnowledgeError::AdapterUnavailable("HNSW cache panicked while rebuilding"));
    let basename = match build {
        Ok(Ok(basename)) => basename,
        Ok(Err(error)) | Err(error) => {
            let _ = std::fs::remove_dir_all(&temporary);
            return Err(error);
        }
    };
    let manifest = Manifest {
        format_version: FORMAT_VERSION,
        signature: signature.into(),
        id_scheme: id_scheme.into(),
        dimensions,
        chunk_ids: points.iter().map(|(id, _)| id.clone()).collect(),
        basename,
    };
    if let Err(error) = validate_manifest(&manifest, signature, id_scheme, dimensions) {
        let _ = std::fs::remove_dir_all(&temporary);
        return Err(error);
    }
    let manifest_bytes = match serde_json::to_vec_pretty(&manifest) {
        Ok(bytes) if bytes.len() <= MAX_MANIFEST_BYTES as usize => bytes,
        Ok(_) => {
            let _ = std::fs::remove_dir_all(&temporary);
            return Err(KnowledgeError::InvalidInput(
                "HNSW manifest exceeds size limit",
            ));
        }
        Err(error) => {
            let _ = std::fs::remove_dir_all(&temporary);
            return Err(failure(format!("cannot encode HNSW manifest: {error}")));
        }
    };
    if let Err(error) = std::fs::write(manifest_path(&temporary), manifest_bytes) {
        let _ = std::fs::remove_dir_all(&temporary);
        return Err(failure(format!("cannot write HNSW manifest: {error}")));
    }

    let backup = parent.join(format!(".{partition}.{}.{nonce}.old", std::process::id()));
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
            std::fs::rename(&backup, &directory).map_err(|rollback| {
                failure(format!(
                    "cannot activate HNSW cache ({error}); rollback also failed: {rollback}"
                ))
            })?;
        }
        return Err(failure(format!("cannot activate HNSW cache: {error}")));
    }
    let _ = std::fs::remove_dir_all(backup);
    Ok(true)
}

pub fn search(
    root: &Path,
    signature: &str,
    id_scheme: &str,
    dimensions: usize,
    query: &[f32],
    limit: usize,
) -> Result<Vec<(String, f32)>> {
    with_cache_lock(root, false, || {
        search_locked(root, signature, id_scheme, dimensions, query, limit)
    })
}

fn search_locked(
    root: &Path,
    signature: &str,
    id_scheme: &str,
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
    let directory = index_directory(root, signature, id_scheme, dimensions);
    let manifest = read_manifest(&directory, signature, id_scheme, dimensions)?;
    catch_unwind(AssertUnwindSafe(|| {
        let mut loader = HnswIo::new(&directory, &manifest.basename);
        let hnsw: Hnsw<f32, DistCosine> = loader
            .load_hnsw()
            .map_err(|error| failure(format!("cannot load HNSW cache: {error}")))?;
        if hnsw.get_nb_point() != manifest.chunk_ids.len() {
            return Err(KnowledgeError::IncompatibleIndex(
                "HNSW point count does not match manifest",
            ));
        }
        if hnsw.get_point_indexation().get_data_dimension() != dimensions {
            return Err(KnowledgeError::IncompatibleIndex(
                "HNSW data dimensions do not match manifest",
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
    }))
    .map_err(|_| KnowledgeError::IncompatibleIndex("HNSW cache panicked while loading"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    const SCHEME: &str = "sha256-v1";

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
        assert!(rebuild(&root, "test-signature", SCHEME, 16, &points).unwrap());
        assert!(is_available(&root, "test-signature", SCHEME, 16));
        let result = search(&root, "test-signature", SCHEME, 16, &points[173].1, 10).unwrap();
        assert_eq!(result[0].0, "chunk-173");
        clear(&root).unwrap();
        assert!(!is_available(&root, "test-signature", SCHEME, 16));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn too_few_points_remove_stale_partition() {
        let root = temp_root("small");
        let points = points();
        rebuild(&root, "test-signature", SCHEME, 16, &points).unwrap();
        assert!(!rebuild(&root, "test-signature", SCHEME, 16, &points[..2]).unwrap());
        assert!(!is_available(&root, "test-signature", SCHEME, 16));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn non_finite_vectors_are_rejected_before_hnsw() {
        let root = temp_root("non_finite");
        let mut points = points();
        points[0].1[0] = f32::NAN;
        assert!(matches!(
            rebuild(&root, "test-signature", SCHEME, 16, &points),
            Err(KnowledgeError::InvalidInput(_))
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn corrupt_mismatch_and_count_do_not_become_available() {
        let root = temp_root("corrupt");
        let directory = index_directory(&root, "test-signature", SCHEME, 16);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(manifest_path(&directory), b"not-json").unwrap();
        assert!(!is_available(&root, "test-signature", SCHEME, 16));

        let points = points();
        rebuild(&root, "test-signature", SCHEME, 16, &points).unwrap();
        assert!(!is_available(&root, "other-signature", SCHEME, 16));
        let mut manifest = read_manifest(&directory, "test-signature", SCHEME, 16).unwrap();
        manifest.chunk_ids.pop();
        std::fs::write(
            manifest_path(&directory),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert!(search(&root, "test-signature", SCHEME, 16, &points[0].1, 1).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn corrupt_index_files_return_error_instead_of_panicking() {
        let root = temp_root("corrupt_index");
        let points = points();
        rebuild(&root, "test-signature", SCHEME, 16, &points).unwrap();
        let directory = index_directory(&root, "test-signature", SCHEME, 16);
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            if path.file_name().and_then(|name| name.to_str()) != Some("manifest.json") {
                std::fs::write(path, b"corrupt-index").unwrap();
            }
        }
        assert!(!is_available(&root, "test-signature", SCHEME, 16));
        assert!(search(&root, "test-signature", SCHEME, 16, &points[0].1, 1).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn concurrent_rebuilds_use_distinct_staging_directories() {
        let root = Arc::new(temp_root("concurrent"));
        let points = Arc::new(points());
        let barrier = Arc::new(Barrier::new(3));
        let handles = (0..2)
            .map(|_| {
                let root = Arc::clone(&root);
                let points = Arc::clone(&points);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    rebuild(&root, "test-signature", SCHEME, 16, &points)
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        for handle in handles {
            assert!(handle.join().unwrap().unwrap());
        }
        let result = search(&root, "test-signature", SCHEME, 16, &points[173].1, 1).unwrap();
        assert_eq!(result[0].0, "chunk-173");
        let _ = std::fs::remove_dir_all(root.as_ref());
    }

    #[test]
    fn interrupted_replacement_restores_backup_on_next_exclusive_lock() {
        let root = temp_root("recovery");
        let points = points();
        rebuild(&root, "test-signature", SCHEME, 16, &points).unwrap();
        let directory = index_directory(&root, "test-signature", SCHEME, 16);
        let partition = partition_name("test-signature", SCHEME, 16);
        let backup = directory
            .parent()
            .unwrap()
            .join(format!(".{partition}.999.1.old"));
        std::fs::rename(&directory, &backup).unwrap();
        assert!(!directory.exists());
        assert!(is_available(&root, "test-signature", SCHEME, 16));
        assert!(directory.exists());
        assert!(!backup.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn oversized_manifest_is_rejected_before_reading() {
        let root = temp_root("oversized");
        let directory = index_directory(&root, "test-signature", SCHEME, 16);
        std::fs::create_dir_all(&directory).unwrap();
        let file = File::create(manifest_path(&directory)).unwrap();
        file.set_len(MAX_MANIFEST_BYTES + 1).unwrap();
        assert!(!is_available(&root, "test-signature", SCHEME, 16));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_partition_hash_is_stable() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../fixtures/legacy-hnsw-v1.json")).unwrap();
        let signature = fixture["signature"].as_str().unwrap();
        let dimensions = fixture["dimensions"].as_u64().unwrap() as usize;
        assert_eq!(
            partition_name(signature, "", dimensions),
            fixture["partition"].as_str().unwrap()
        );
    }

    #[test]
    fn id_scheme_mismatch_makes_stale_partition_unusable() {
        let root = temp_root("scheme_mismatch");
        let points = points();
        rebuild(&root, "test-signature", "", 16, &points).unwrap();
        assert!(is_available(&root, "test-signature", "", 16));
        // New SQLite ID scheme must not resolve/use the legacy ANN partition.
        assert!(!is_available(&root, "test-signature", SCHEME, 16));
        assert!(search(&root, "test-signature", SCHEME, 16, &points[0].1, 1).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn persisted_manifest_rejects_missing_or_wrong_id_scheme() {
        let root = temp_root("manifest_scheme");
        let points = points();
        rebuild(&root, "test-signature", SCHEME, 16, &points).unwrap();
        let directory = index_directory(&root, "test-signature", SCHEME, 16);
        let mut manifest = read_manifest(&directory, "test-signature", SCHEME, 16).unwrap();
        manifest.id_scheme.clear();
        std::fs::write(
            manifest_path(&directory),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert!(!is_available(&root, "test-signature", SCHEME, 16));
        manifest.id_scheme = "sip13-v1".into();
        std::fs::write(
            manifest_path(&directory),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert!(!is_available(&root, "test-signature", SCHEME, 16));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_binary_fixture_opens_and_searches() {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/legacy-hnsw-index-v1/.markhand/vector-index")
            .join(partition_name("local_hash_v1", "", 256));
        let root = temp_root("legacy_fixture");
        let target = index_directory(&root, "local_hash_v1", "", 256);
        std::fs::create_dir_all(&target).unwrap();
        for name in ["manifest.json", "index.hnsw.data", "index.hnsw.graph"] {
            std::fs::copy(fixture.join(name), target.join(name)).unwrap();
        }
        let mut query = vec![0.0; 256];
        query[0] = 1.0;
        assert!(is_available(&root, "local_hash_v1", "", 256));
        let result = search(&root, "local_hash_v1", "", 256, &query, 3).unwrap();
        assert_eq!(result[0].0, "legacy-chunk-000");
        let _ = std::fs::remove_dir_all(root);
    }
}
