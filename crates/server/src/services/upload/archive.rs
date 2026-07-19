//! ZIP/OOXML archive safety: bomb, traversal, nested, required members.
//!
//! Compression-ratio and uncompressed caps are enforced from the ZIP **central
//! directory** declared sizes (no unbounded decompression). Membership checks
//! read individual entries through a bounded decompressor.

use std::io::{Read, Seek};
use std::path::Path;

use zip::read::ZipFile;
use zip::result::ZipError;
use zip::ZipArchive;

use super::error::{ReasonCode, ThreatClass, UploadError};
use super::limits::{LimitsConfig, MAX_SINGLE_ENTRY_READ_BYTES};
use super::sniff::{refine_zip_format, CanonicalFormat};

const CONTENT_TYPES: &str = "[Content_Types].xml";
const WORD_DOCUMENT: &str = "word/document.xml";
const PPT_PRESENTATION: &str = "ppt/presentation.xml";
const XL_WORKBOOK: &str = "xl/workbook.xml";
const ODS_MIMETYPE: &str = "mimetype";

const NESTED_ARCHIVE_SUFFIXES: &[&str] = &[
    ".zip", ".jar", ".7z", ".rar", ".gz", ".tgz", ".tar", ".bz2", ".xz", ".docx", ".pptx", ".xlsx",
    ".ods",
];

/// Result of archive preflight for ZIP-based formats.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveCheck {
    pub format: CanonicalFormat,
    pub entry_count: u64,
    pub uncompressed_bytes: u64,
    pub compressed_bytes: u64,
}

/// Validate a ZIP/OOXML/ODS archive without unbounded decompression.
pub fn validate_zip_archive(
    path: &Path,
    provisional: CanonicalFormat,
    limits: &LimitsConfig,
) -> Result<ArchiveCheck, UploadError> {
    let file = std::fs::File::open(path).map_err(|_| {
        UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
    })?;
    validate_zip_reader(file, provisional, limits)
}

fn validate_zip_reader<R: Read + Seek>(
    reader: R,
    provisional: CanonicalFormat,
    limits: &LimitsConfig,
) -> Result<ArchiveCheck, UploadError> {
    let mut archive = ZipArchive::new(reader).map_err(map_zip_open_error)?;
    let entry_count = archive.len() as u64;
    if entry_count == 0 {
        return Err(UploadError::rejected(
            ThreatClass::MalformedOoxml,
            ReasonCode::MalformedArchive,
        ));
    }
    if entry_count > limits.max_archive_entries {
        return Err(UploadError::rejected(
            ThreatClass::ArchiveBomb,
            ReasonCode::ArchiveEntryLimit,
        ));
    }

    let mut uncompressed_bytes: u64 = 0;
    let mut compressed_bytes: u64 = 0;
    let mut has_content_types = false;
    let mut has_word = false;
    let mut has_ppt = false;
    let mut has_xl = false;
    let mut has_ods_mimetype = false;

    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(map_zip_entry_error)?;
        let name = entry.name().to_string();
        reject_dangerous_entry_name(&name)?;
        reject_nested_archive_name(&name)?;

        let declared_uncomp = entry.size();
        let declared_comp = entry.compressed_size();
        uncompressed_bytes = uncompressed_bytes.saturating_add(declared_uncomp);
        compressed_bytes = compressed_bytes.saturating_add(declared_comp);

        if uncompressed_bytes > limits.max_archive_uncompressed_bytes {
            return Err(UploadError::rejected(
                ThreatClass::ArchiveBomb,
                ReasonCode::ArchiveUncompressedLimit,
            ));
        }

        if name == CONTENT_TYPES {
            has_content_types = true;
        }
        if name == WORD_DOCUMENT || name.starts_with("word/") {
            has_word = true;
        }
        if name == PPT_PRESENTATION || name.starts_with("ppt/") {
            has_ppt = true;
        }
        if name == XL_WORKBOOK || name.starts_with("xl/") {
            has_xl = true;
        }
        if name == ODS_MIMETYPE {
            has_ods_mimetype = true;
        }
        drop(entry);
    }

    // Ratio from central-directory declared sizes — no full decompress.
    if compressed_bytes > 0 {
        let ratio = uncompressed_bytes / compressed_bytes;
        if ratio > limits.max_archive_compression_ratio {
            return Err(UploadError::rejected(
                ThreatClass::ArchiveBomb,
                ReasonCode::ArchiveCompressionRatio,
            ));
        }
    } else if uncompressed_bytes > 0 {
        return Err(UploadError::rejected(
            ThreatClass::ArchiveBomb,
            ReasonCode::ArchiveCompressionRatio,
        ));
    }

    let format = refine_zip_format(
        provisional,
        has_content_types,
        has_word,
        has_ppt,
        has_xl,
        has_ods_mimetype,
    )?;

    // Require canonical member paths for OOXML.
    match format {
        CanonicalFormat::Docx if !has_content_types || !has_word => {
            return Err(UploadError::rejected(
                ThreatClass::MalformedOoxml,
                ReasonCode::MissingFormatPaths,
            ));
        }
        CanonicalFormat::Pptx if !has_content_types || !has_ppt => {
            return Err(UploadError::rejected(
                ThreatClass::MalformedOoxml,
                ReasonCode::MissingFormatPaths,
            ));
        }
        CanonicalFormat::Xlsx if !has_content_types || !has_xl => {
            return Err(UploadError::rejected(
                ThreatClass::MalformedOoxml,
                ReasonCode::MissingFormatPaths,
            ));
        }
        CanonicalFormat::Ods if !has_ods_mimetype => {
            return Err(UploadError::rejected(
                ThreatClass::MalformedOoxml,
                ReasonCode::MissingFormatPaths,
            ));
        }
        _ => {}
    }

    // Bounded well-formedness checks on required XML (fail closed).
    verify_required_xml(&mut archive, format)?;

    Ok(ArchiveCheck {
        format,
        entry_count,
        uncompressed_bytes,
        compressed_bytes,
    })
}

fn verify_required_xml<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    format: CanonicalFormat,
) -> Result<(), UploadError> {
    let required: &[&str] = match format {
        CanonicalFormat::Docx => &[CONTENT_TYPES, WORD_DOCUMENT],
        CanonicalFormat::Pptx => &[CONTENT_TYPES, PPT_PRESENTATION],
        CanonicalFormat::Xlsx => &[CONTENT_TYPES, XL_WORKBOOK],
        CanonicalFormat::Ods => &["content.xml", "META-INF/manifest.xml"],
        _ => return Ok(()),
    };
    for name in required {
        let mut entry = match archive.by_name(name) {
            Ok(entry) => entry,
            Err(ZipError::FileNotFound) => {
                return Err(UploadError::rejected(
                    ThreatClass::MalformedOoxml,
                    ReasonCode::MissingFormatPaths,
                ));
            }
            Err(_) => {
                return Err(UploadError::rejected(
                    ThreatClass::MalformedOoxml,
                    ReasonCode::MalformedArchive,
                ));
            }
        };
        let bytes = read_entry_bounded(&mut entry)?;
        if name.ends_with(".xml") && !looks_like_well_formed_xml(&bytes) {
            return Err(UploadError::rejected(
                ThreatClass::MalformedOoxml,
                ReasonCode::MalformedXml,
            ));
        }
    }
    Ok(())
}

/// Read an entry with a hard uncompressed byte bound (zip-bomb defense).
fn read_entry_bounded(entry: &mut ZipFile<'_>) -> Result<Vec<u8>, UploadError> {
    let declared = entry.size();
    if declared > MAX_SINGLE_ENTRY_READ_BYTES {
        return Err(UploadError::rejected(
            ThreatClass::ArchiveBomb,
            ReasonCode::ArchiveUncompressedLimit,
        ));
    }
    let mut buf = Vec::new();
    let mut limited = entry.take(MAX_SINGLE_ENTRY_READ_BYTES + 1);
    limited
        .read_to_end(&mut buf)
        .map_err(|_| UploadError::rejected(ThreatClass::MalformedOoxml, ReasonCode::FailClosed))?;
    if buf.len() as u64 > MAX_SINGLE_ENTRY_READ_BYTES {
        return Err(UploadError::rejected(
            ThreatClass::ArchiveBomb,
            ReasonCode::ArchiveUncompressedLimit,
        ));
    }
    Ok(buf)
}

fn looks_like_well_formed_xml(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let trimmed = text.trim();
    if !trimmed.starts_with('<') || !trimmed.ends_with('>') {
        return false;
    }
    // Cheap balance check: opens should not wildly exceed closes (truncation).
    let open = trimmed.matches('<').count();
    let close = trimmed.matches("</").count() + trimmed.matches("/>").count();
    open > 0 && close > 0 && open <= close * 3
}

pub fn reject_dangerous_entry_name(name: &str) -> Result<(), UploadError> {
    if name.is_empty()
        || name.contains('\0')
        || name.contains('\\')
        || name.starts_with('/')
        || name.starts_with('\\')
        || name.contains("../")
        || name.contains("..\\")
        || name.split('/').any(|part| part == "..")
        || name.contains(':')
    // drive / URL schemes
    {
        return Err(UploadError::rejected(
            ThreatClass::ArchiveTraversal,
            ReasonCode::ArchivePathTraversal,
        ));
    }
    Ok(())
}

fn reject_nested_archive_name(name: &str) -> Result<(), UploadError> {
    // Allow the OOXML package members; reject nested archive-looking names.
    let lower = name.to_ascii_lowercase();
    // Skip directory entries.
    if lower.ends_with('/') {
        return Ok(());
    }
    let base = lower.rsplit('/').next().unwrap_or(lower.as_str());
    // Nested archives inside word/media etc.
    if NESTED_ARCHIVE_SUFFIXES
        .iter()
        .any(|suffix| base.ends_with(suffix))
    {
        // The outer package is the format; members named *.docx etc. are nested.
        return Err(UploadError::rejected(
            ThreatClass::NestedArchive,
            ReasonCode::NestedArchiveEntry,
        ));
    }
    Ok(())
}

fn map_zip_open_error(error: ZipError) -> UploadError {
    match error {
        ZipError::InvalidArchive(_) | ZipError::UnsupportedArchive(_) => {
            UploadError::rejected(ThreatClass::MalformedOoxml, ReasonCode::MalformedArchive)
        }
        _ => UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed),
    }
}

fn map_zip_entry_error(_error: ZipError) -> UploadError {
    UploadError::rejected(ThreatClass::MalformedOoxml, ReasonCode::MalformedArchive)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    use zip::CompressionMethod;

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        for (name, data) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn rejects_traversal_names() {
        assert!(reject_dangerous_entry_name("../../etc/passwd").is_err());
        assert!(reject_dangerous_entry_name("/absolute/path").is_err());
        assert!(reject_dangerous_entry_name("C:/windows/system32").is_err());
        assert!(reject_dangerous_entry_name("word/document.xml").is_ok());
    }

    #[test]
    fn rejects_entry_count_bomb_without_decompressing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bomb.zip");
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        // Include minimal OOXML markers so we fail on entry count first.
        zip.start_file(CONTENT_TYPES, options).unwrap();
        zip.write_all(b"<?xml version=\"1.0\"?><Types></Types>")
            .unwrap();
        zip.start_file(WORD_DOCUMENT, options).unwrap();
        zip.write_all(b"<?xml version=\"1.0\"?><w:document></w:document>")
            .unwrap();
        for i in 0..5_000 {
            zip.start_file(format!("pad/{i}.txt"), options).unwrap();
            zip.write_all(b"x").unwrap();
        }
        zip.finish().unwrap();

        let limits = LimitsConfig::policy_defaults();
        let err = validate_zip_archive(&path, CanonicalFormat::Docx, &limits).unwrap_err();
        assert_eq!(err.threat_class(), Some(ThreatClass::ArchiveBomb));
        assert_eq!(err.reason_code(), ReasonCode::ArchiveEntryLimit);
    }

    #[test]
    fn accepts_minimal_docx() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ok.docx");
        write_zip(
            &path,
            &[
                (
                    CONTENT_TYPES,
                    br#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"></Types>"#,
                ),
                (
                    WORD_DOCUMENT,
                    br#"<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"></w:document>"#,
                ),
            ],
        );
        let limits = LimitsConfig::policy_defaults();
        let check = validate_zip_archive(&path, CanonicalFormat::Docx, &limits).unwrap();
        assert_eq!(check.format, CanonicalFormat::Docx);
    }
}
