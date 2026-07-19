//! ZIP/OOXML archive safety: bomb, traversal, nested, required members.
//!
//! Every member is inflated through a fixed-size buffer. Central-directory sizes
//! are treated as claims to verify, not as authority for policy decisions.

use std::collections::HashSet;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use zip::read::ZipFile;
use zip::result::ZipError;
use zip::ZipArchive;

use super::error::{ReasonCode, ThreatClass, UploadError};
use super::limits::{LimitsConfig, MAX_SINGLE_ENTRY_READ_BYTES, STREAM_CHUNK_BYTES};
use super::sniff::{refine_zip_format, CanonicalFormat};

const CONTENT_TYPES: &str = "[Content_Types].xml";
const WORD_DOCUMENT: &str = "word/document.xml";
const PPT_PRESENTATION: &str = "ppt/presentation.xml";
const XL_WORKBOOK: &str = "xl/workbook.xml";
const ODS_MIMETYPE: &str = "mimetype";

const ODS_SPREADSHEET_MIMETYPE: &[u8] = b"application/vnd.oasis.opendocument.spreadsheet";
const NESTED_MAGIC_BYTES: usize = 512;

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

pub(crate) fn validate_zip_reader<R: Read + Seek>(
    mut reader: R,
    provisional: CanonicalFormat,
    limits: &LimitsConfig,
) -> Result<ArchiveCheck, UploadError> {
    reject_duplicate_central_directory_names(&mut reader)?;
    reader.seek(SeekFrom::Start(0)).map_err(|_| {
        UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
    })?;
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
    let mut content_types = ContentTypesSeen::default();
    let mut seen_names = HashSet::new();

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(map_zip_entry_error)?;
        let name = entry.name().to_string();
        reject_dangerous_entry_name(&name)?;
        let normalized = name.trim_end_matches('/').to_string();
        if !seen_names.insert(normalized) {
            return Err(UploadError::rejected(
                ThreatClass::MalformedOoxml,
                ReasonCode::MalformedArchive,
            ));
        }
        if entry.is_symlink() {
            return Err(UploadError::rejected(
                ThreatClass::ArchiveTraversal,
                ReasonCode::ArchivePathTraversal,
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

        let scanned = if entry.is_dir() {
            EntryScan::default()
        } else if name == CONTENT_TYPES {
            scan_content_types_xml(
                &mut entry,
                limits,
                &mut uncompressed_bytes,
                &mut compressed_bytes,
            )?
        } else if matches!(
            name.as_str(),
            WORD_DOCUMENT | PPT_PRESENTATION | XL_WORKBOOK
        ) || (matches!(provisional, CanonicalFormat::Ods)
            && matches!(name.as_str(), "content.xml" | "META-INF/manifest.xml"))
        {
            scan_required_xml(
                &mut entry,
                limits,
                &mut uncompressed_bytes,
                &mut compressed_bytes,
            )?
        } else if name == ODS_MIMETYPE {
            scan_ods_mimetype(
                &mut entry,
                limits,
                &mut uncompressed_bytes,
                &mut compressed_bytes,
            )?
        } else {
            drain_checked_entry(
                &mut entry,
                limits,
                &mut uncompressed_bytes,
                &mut compressed_bytes,
            )?
        };
        if name == CONTENT_TYPES {
            content_types = scanned.content_types;
        }
        if name == ODS_MIMETYPE && !scanned.ods_mimetype_ok {
            return Err(UploadError::rejected(
                ThreatClass::MalformedOoxml,
                ReasonCode::MissingFormatPaths,
            ));
        }
    }

    if [has_word, has_ppt, has_xl]
        .into_iter()
        .filter(|present| *present)
        .count()
        > 1
    {
        return Err(UploadError::rejected(
            ThreatClass::MalformedOoxml,
            ReasonCode::MissingFormatPaths,
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

    verify_content_types(format, &content_types)?;

    Ok(ArchiveCheck {
        format,
        entry_count,
        uncompressed_bytes,
        compressed_bytes,
    })
}

#[derive(Debug, Default, Clone, Copy)]
struct ContentTypesSeen {
    word_document: bool,
    ppt_presentation: bool,
    xl_workbook: bool,
}

#[derive(Debug, Default)]
struct EntryScan {
    content_types: ContentTypesSeen,
    ods_mimetype_ok: bool,
}

fn scan_content_types_xml(
    entry: &mut ZipFile<'_>,
    limits: &LimitsConfig,
    aggregate_uncompressed: &mut u64,
    aggregate_compressed: &mut u64,
) -> Result<EntryScan, UploadError> {
    let mut checked =
        CheckedEntryReader::new(entry, limits, aggregate_uncompressed, aggregate_compressed);
    let content_types = parse_content_types_xml(&mut checked)?;
    checked.drain_to_end()?;
    checked.finish()?;
    Ok(EntryScan {
        content_types,
        ods_mimetype_ok: false,
    })
}

fn scan_required_xml(
    entry: &mut ZipFile<'_>,
    limits: &LimitsConfig,
    aggregate_uncompressed: &mut u64,
    aggregate_compressed: &mut u64,
) -> Result<EntryScan, UploadError> {
    let mut checked =
        CheckedEntryReader::new(entry, limits, aggregate_uncompressed, aggregate_compressed);
    parse_well_formed_xml(&mut checked)?;
    checked.drain_to_end()?;
    checked.finish()?;
    Ok(EntryScan::default())
}

fn scan_ods_mimetype(
    entry: &mut ZipFile<'_>,
    limits: &LimitsConfig,
    aggregate_uncompressed: &mut u64,
    aggregate_compressed: &mut u64,
) -> Result<EntryScan, UploadError> {
    let mut checked =
        CheckedEntryReader::new(entry, limits, aggregate_uncompressed, aggregate_compressed);
    let mut actual = Vec::with_capacity(ODS_SPREADSHEET_MIMETYPE.len());
    let mut buf = [0_u8; STREAM_CHUNK_BYTES];
    loop {
        let n = checked.read(&mut buf).map_err(map_entry_read_error)?;
        if n == 0 {
            break;
        }
        let remaining = ODS_SPREADSHEET_MIMETYPE.len().saturating_sub(actual.len());
        if remaining > 0 {
            actual.extend_from_slice(&buf[..n.min(remaining)]);
        }
    }
    checked.finish()?;
    Ok(EntryScan {
        content_types: ContentTypesSeen::default(),
        ods_mimetype_ok: actual == ODS_SPREADSHEET_MIMETYPE,
    })
}

fn drain_checked_entry(
    entry: &mut ZipFile<'_>,
    limits: &LimitsConfig,
    aggregate_uncompressed: &mut u64,
    aggregate_compressed: &mut u64,
) -> Result<EntryScan, UploadError> {
    let mut checked =
        CheckedEntryReader::new(entry, limits, aggregate_uncompressed, aggregate_compressed);
    checked.drain_to_end()?;
    checked.finish()?;
    Ok(EntryScan::default())
}

struct CheckedEntryReader<'a, 'b> {
    entry: &'a mut ZipFile<'b>,
    limits: &'a LimitsConfig,
    aggregate_uncompressed: &'a mut u64,
    aggregate_compressed: &'a mut u64,
    declared_uncompressed: u64,
    declared_compressed: u64,
    declared_crc32: u32,
    actual_uncompressed: u64,
    crc32: crc32fast::Hasher,
    first_bytes: Vec<u8>,
}

impl<'a, 'b> CheckedEntryReader<'a, 'b> {
    fn new(
        entry: &'a mut ZipFile<'b>,
        limits: &'a LimitsConfig,
        aggregate_uncompressed: &'a mut u64,
        aggregate_compressed: &'a mut u64,
    ) -> Self {
        Self {
            declared_uncompressed: entry.size(),
            declared_compressed: entry.compressed_size(),
            declared_crc32: entry.crc32(),
            entry,
            limits,
            aggregate_uncompressed,
            aggregate_compressed,
            actual_uncompressed: 0,
            crc32: crc32fast::Hasher::new(),
            first_bytes: Vec::with_capacity(NESTED_MAGIC_BYTES),
        }
    }

    fn drain_to_end(&mut self) -> Result<(), UploadError> {
        let mut buf = [0_u8; STREAM_CHUNK_BYTES];
        loop {
            let n = self.read(&mut buf).map_err(map_entry_read_error)?;
            if n == 0 {
                break;
            }
        }
        Ok(())
    }

    fn finish(self) -> Result<(), UploadError> {
        if looks_like_nested_archive_magic(&self.first_bytes) {
            return Err(UploadError::rejected(
                ThreatClass::NestedArchive,
                ReasonCode::NestedArchiveEntry,
            ));
        }
        if self.actual_uncompressed != self.declared_uncompressed {
            return Err(UploadError::rejected(
                ThreatClass::MalformedOoxml,
                ReasonCode::MalformedArchive,
            ));
        }
        if self.crc32.finalize() != self.declared_crc32 {
            return Err(UploadError::rejected(
                ThreatClass::MalformedOoxml,
                ReasonCode::MalformedArchive,
            ));
        }
        Ok(())
    }
}

impl Read for CheckedEntryReader<'_, '_> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let n = self.entry.read(out)?;
        if n == 0 {
            return Ok(0);
        }
        let chunk = &out[..n];
        if self.first_bytes.len() < NESTED_MAGIC_BYTES {
            let need = NESTED_MAGIC_BYTES - self.first_bytes.len();
            self.first_bytes
                .extend_from_slice(&chunk[..chunk.len().min(need)]);
        }
        self.actual_uncompressed = self.actual_uncompressed.saturating_add(n as u64);
        *self.aggregate_uncompressed = self.aggregate_uncompressed.saturating_add(n as u64);
        if self.actual_uncompressed == n as u64 {
            *self.aggregate_compressed = self
                .aggregate_compressed
                .saturating_add(self.declared_compressed);
            if self.declared_compressed == 0 && self.declared_uncompressed > 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid compression ratio",
                ));
            }
        }
        self.crc32.update(chunk);
        enforce_archive_limits(
            self.limits,
            self.actual_uncompressed,
            self.declared_compressed,
            *self.aggregate_uncompressed,
            *self.aggregate_compressed,
        )
        .map_err(|error| {
            let message = match error.reason_code() {
                ReasonCode::ArchiveCompressionRatio => "archive compression ratio",
                _ => "archive uncompressed limit",
            };
            std::io::Error::new(std::io::ErrorKind::InvalidData, message)
        })?;
        Ok(n)
    }
}

fn enforce_archive_limits(
    limits: &LimitsConfig,
    entry_uncompressed: u64,
    entry_compressed: u64,
    aggregate_uncompressed: u64,
    aggregate_compressed: u64,
) -> Result<(), UploadError> {
    if entry_uncompressed > MAX_SINGLE_ENTRY_READ_BYTES
        || aggregate_uncompressed > limits.max_archive_uncompressed_bytes
    {
        return Err(UploadError::rejected(
            ThreatClass::ArchiveBomb,
            ReasonCode::ArchiveUncompressedLimit,
        ));
    }
    if (entry_compressed > 0
        && entry_uncompressed
            > entry_compressed.saturating_mul(limits.max_archive_compression_ratio))
        || (aggregate_compressed > 0
            && aggregate_uncompressed
                > aggregate_compressed.saturating_mul(limits.max_archive_compression_ratio))
    {
        return Err(UploadError::rejected(
            ThreatClass::ArchiveBomb,
            ReasonCode::ArchiveCompressionRatio,
        ));
    }
    Ok(())
}

fn parse_content_types_xml<R: Read>(reader: R) -> Result<ContentTypesSeen, UploadError> {
    let mut xml =
        quick_xml::Reader::from_reader(BufReader::with_capacity(STREAM_CHUNK_BYTES, reader));
    xml.config_mut().trim_text(true);
    let mut buf = Vec::with_capacity(1024);
    let mut seen = ContentTypesSeen::default();
    let mut saw_event = false;
    loop {
        match xml.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Start(event))
            | Ok(quick_xml::events::Event::Empty(event)) => {
                saw_event = true;
                let mut part_name = None;
                let mut content_type = None;
                for attr in event.attributes().flatten() {
                    match attr.key.as_ref() {
                        b"PartName" => part_name = Some(attr.value.into_owned()),
                        b"ContentType" => content_type = Some(attr.value.into_owned()),
                        _ => {}
                    }
                }
                if let (Some(part), Some(kind)) = (part_name, content_type) {
                    let part = String::from_utf8_lossy(&part).to_ascii_lowercase();
                    let kind = String::from_utf8_lossy(&kind).to_ascii_lowercase();
                    seen.word_document |= part == "/word/document.xml"
                        && kind.contains("wordprocessingml.document.main+xml");
                    seen.ppt_presentation |= part == "/ppt/presentation.xml"
                        && kind.contains("presentationml.presentation.main+xml");
                    seen.xl_workbook |=
                        part == "/xl/workbook.xml" && kind.contains("spreadsheetml.sheet.main+xml");
                }
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Ok(_) => {}
            Err(_) => {
                return Err(UploadError::rejected(
                    ThreatClass::MalformedOoxml,
                    ReasonCode::MalformedXml,
                ));
            }
        }
        buf.clear();
    }
    if !saw_event {
        return Err(UploadError::rejected(
            ThreatClass::MalformedOoxml,
            ReasonCode::MalformedXml,
        ));
    }
    Ok(seen)
}

fn parse_well_formed_xml<R: Read>(reader: R) -> Result<(), UploadError> {
    let mut xml =
        quick_xml::Reader::from_reader(BufReader::with_capacity(STREAM_CHUNK_BYTES, reader));
    xml.config_mut().trim_text(true);
    let mut buf = Vec::with_capacity(1024);
    let mut saw_event = false;
    loop {
        match xml.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Start(_))
            | Ok(quick_xml::events::Event::Empty(_))
            | Ok(quick_xml::events::Event::Text(_)) => saw_event = true,
            Ok(quick_xml::events::Event::Eof) => break,
            Ok(_) => {}
            Err(_) => {
                return Err(UploadError::rejected(
                    ThreatClass::MalformedOoxml,
                    ReasonCode::MalformedXml,
                ));
            }
        }
        buf.clear();
    }
    if !saw_event {
        return Err(UploadError::rejected(
            ThreatClass::MalformedOoxml,
            ReasonCode::MalformedXml,
        ));
    }
    Ok(())
}

fn verify_content_types(
    format: CanonicalFormat,
    content_types: &ContentTypesSeen,
) -> Result<(), UploadError> {
    let ok = match format {
        CanonicalFormat::Docx => content_types.word_document,
        CanonicalFormat::Pptx => content_types.ppt_presentation,
        CanonicalFormat::Xlsx => content_types.xl_workbook,
        CanonicalFormat::Ods => true,
        _ => true,
    };
    if ok {
        Ok(())
    } else {
        Err(UploadError::rejected(
            ThreatClass::MalformedOoxml,
            ReasonCode::MissingFormatPaths,
        ))
    }
}

fn looks_like_nested_archive_magic(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04")
        || bytes.starts_with(b"PK\x05\x06")
        || bytes.starts_with(b"PK\x07\x08")
        || bytes.starts_with(b"Rar!\x1a\x07")
        || bytes.starts_with(b"7z\xbc\xaf\x27\x1c")
        || bytes.starts_with(&[0x1f, 0x8b])
        || bytes.starts_with(b"BZh")
        || bytes.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0x00])
        || bytes.get(257..263).is_some_and(|magic| magic == b"ustar\0")
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

fn reject_duplicate_central_directory_names<R: Read + Seek>(
    reader: &mut R,
) -> Result<(), UploadError> {
    let len = reader.seek(SeekFrom::End(0)).map_err(|_| {
        UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
    })?;
    let tail_len = len.min(66 * 1024) as usize;
    reader
        .seek(SeekFrom::End(-(tail_len as i64)))
        .map_err(|_| {
            UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
        })?;
    let mut tail = vec![0_u8; tail_len];
    reader.read_exact(&mut tail).map_err(|_| {
        UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
    })?;
    let Some(eocd_pos) = tail.windows(4).rposition(|w| w == b"PK\x05\x06") else {
        return Ok(());
    };
    if eocd_pos + 22 > tail.len() {
        return Ok(());
    }
    let entries = u16::from_le_bytes(tail[eocd_pos + 10..eocd_pos + 12].try_into().unwrap());
    let central_offset =
        u32::from_le_bytes(tail[eocd_pos + 16..eocd_pos + 20].try_into().unwrap()) as u64;
    if entries == u16::MAX {
        return Ok(());
    }
    reader.seek(SeekFrom::Start(central_offset)).map_err(|_| {
        UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
    })?;
    let mut seen = HashSet::new();
    for _ in 0..entries {
        let mut header = [0_u8; 46];
        reader.read_exact(&mut header).map_err(|_| {
            UploadError::rejected(ThreatClass::MalformedOoxml, ReasonCode::MalformedArchive)
        })?;
        if &header[0..4] != b"PK\x01\x02" {
            return Ok(());
        }
        let name_len = u16::from_le_bytes(header[28..30].try_into().unwrap()) as usize;
        let extra_len = u16::from_le_bytes(header[30..32].try_into().unwrap()) as u64;
        let comment_len = u16::from_le_bytes(header[32..34].try_into().unwrap()) as u64;
        if name_len == 0 || name_len > 4096 {
            return Err(UploadError::rejected(
                ThreatClass::MalformedOoxml,
                ReasonCode::MalformedArchive,
            ));
        }
        let mut name = vec![0_u8; name_len];
        reader.read_exact(&mut name).map_err(|_| {
            UploadError::rejected(ThreatClass::MalformedOoxml, ReasonCode::MalformedArchive)
        })?;
        let name = String::from_utf8(name).map_err(|_| {
            UploadError::rejected(ThreatClass::MalformedOoxml, ReasonCode::MalformedArchive)
        })?;
        if !seen.insert(name.trim_end_matches('/').to_string()) {
            return Err(UploadError::rejected(
                ThreatClass::MalformedOoxml,
                ReasonCode::MalformedArchive,
            ));
        }
        reader
            .seek(SeekFrom::Current((extra_len + comment_len) as i64))
            .map_err(|_| {
                UploadError::rejected(ThreatClass::MalformedOoxml, ReasonCode::MalformedArchive)
            })?;
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

fn map_entry_read_error(error: std::io::Error) -> UploadError {
    let message = error.to_string();
    if message.contains("compression") {
        UploadError::rejected(
            ThreatClass::ArchiveBomb,
            ReasonCode::ArchiveCompressionRatio,
        )
    } else if error.kind() == std::io::ErrorKind::InvalidData {
        UploadError::rejected(
            ThreatClass::ArchiveBomb,
            ReasonCode::ArchiveUncompressedLimit,
        )
    } else {
        UploadError::rejected(ThreatClass::MalformedOoxml, ReasonCode::MalformedArchive)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    use zip::CompressionMethod;

    const DOCX_CONTENT_TYPES_XML: &[u8] = br#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;

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
        zip.write_all(DOCX_CONTENT_TYPES_XML).unwrap();
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
                    DOCX_CONTENT_TYPES_XML,
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
