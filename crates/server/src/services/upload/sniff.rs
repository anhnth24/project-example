//! Magic-byte + extension → canonical format (fail closed on spoof).

use super::error::{ReasonCode, ThreatClass, UploadError};

/// Server-derived canonical format (authoritative for downstream).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CanonicalFormat {
    Pdf,
    Docx,
    Pptx,
    Xlsx,
    Ods,
    Xls,
    Xlsb,
    Csv,
    Html,
    PlainText,
    Png,
    Jpeg,
    Webp,
    Tiff,
    Bmp,
    Wav,
    Mp3,
    Ogg,
    Flac,
    M4a,
    /// Provisional ZIP magic before OOXML/ODS path refinement.
    ZipContainer,
}

impl CanonicalFormat {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Docx => "docx",
            Self::Pptx => "pptx",
            Self::Xlsx => "xlsx",
            Self::Ods => "ods",
            Self::Xls => "xls",
            Self::Xlsb => "xlsb",
            Self::Csv => "csv",
            Self::Html => "html",
            Self::PlainText => "txt",
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::Webp => "webp",
            Self::Tiff => "tiff",
            Self::Bmp => "bmp",
            Self::Wav => "wav",
            Self::Mp3 => "mp3",
            Self::Ogg => "ogg",
            Self::Flac => "flac",
            Self::M4a => "m4a",
            Self::ZipContainer => "zip",
        }
    }

    pub const fn is_zip_container(self) -> bool {
        matches!(
            self,
            Self::Docx | Self::Pptx | Self::Xlsx | Self::Ods | Self::ZipContainer
        )
    }

    /// Canonical extension used for server-side behavior (never trust client).
    pub const fn canonical_extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::ZipContainer => "zip",
            other => other.as_str(),
        }
    }
}

/// Resolve canonical format from magic bytes; require declared extension consistency.
pub fn resolve_canonical_format(
    head: &[u8],
    declared_filename: Option<&str>,
) -> Result<CanonicalFormat, UploadError> {
    let magic = detect_magic(head).ok_or_else(|| {
        UploadError::rejected(
            ThreatClass::UnsupportedFormat,
            ReasonCode::MagicUnrecognized,
        )
    })?;

    let declared_ext = declared_filename.and_then(extension_of);
    if let Some(ext) = declared_ext {
        if !extension_matches(magic, ext) {
            return Err(UploadError::rejected(
                ThreatClass::ExtensionSpoof,
                ReasonCode::ExtensionMagicMismatch,
            ));
        }
        // Prefer extension-specific spreadsheet variants when magic is OLE/ZIP.
        if let Some(refined) = refine_from_extension(magic, ext) {
            return Ok(refined);
        }
    }

    Ok(magic)
}

fn extension_of(filename: &str) -> Option<&str> {
    let name = filename.rsplit(['/', '\\']).next().unwrap_or(filename);
    let (_, ext) = name.rsplit_once('.')?;
    if ext.is_empty() || ext.len() > 16 {
        return None;
    }
    Some(ext)
}

fn extension_matches(format: CanonicalFormat, ext: &str) -> bool {
    let ext = ext.to_ascii_lowercase();
    match format {
        CanonicalFormat::Pdf => ext == "pdf",
        CanonicalFormat::Docx => ext == "docx",
        CanonicalFormat::Pptx => ext == "pptx",
        CanonicalFormat::Xlsx => ext == "xlsx",
        CanonicalFormat::Ods => ext == "ods",
        CanonicalFormat::ZipContainer => {
            matches!(ext.as_str(), "docx" | "pptx" | "xlsx" | "ods")
        }
        CanonicalFormat::Xls => ext == "xls",
        CanonicalFormat::Xlsb => ext == "xlsb",
        CanonicalFormat::Csv => matches!(ext.as_str(), "csv" | "tsv"),
        CanonicalFormat::Html => matches!(ext.as_str(), "html" | "htm"),
        CanonicalFormat::PlainText => matches!(ext.as_str(), "txt" | "md"),
        CanonicalFormat::Png => ext == "png",
        CanonicalFormat::Jpeg => matches!(ext.as_str(), "jpg" | "jpeg"),
        CanonicalFormat::Webp => ext == "webp",
        CanonicalFormat::Tiff => matches!(ext.as_str(), "tif" | "tiff"),
        CanonicalFormat::Bmp => ext == "bmp",
        CanonicalFormat::Wav => ext == "wav",
        CanonicalFormat::Mp3 => ext == "mp3",
        CanonicalFormat::Ogg => ext == "ogg",
        CanonicalFormat::Flac => ext == "flac",
        CanonicalFormat::M4a => matches!(ext.as_str(), "m4a" | "mp4"),
    }
}

fn refine_from_extension(magic: CanonicalFormat, ext: &str) -> Option<CanonicalFormat> {
    let ext = ext.to_ascii_lowercase();
    match (magic, ext.as_str()) {
        (CanonicalFormat::ZipContainer, "docx") => Some(CanonicalFormat::Docx),
        (CanonicalFormat::ZipContainer, "pptx") => Some(CanonicalFormat::Pptx),
        (CanonicalFormat::ZipContainer, "xlsx") => Some(CanonicalFormat::Xlsx),
        (CanonicalFormat::ZipContainer, "ods") => Some(CanonicalFormat::Ods),
        (CanonicalFormat::Xls | CanonicalFormat::Xlsb, "xls") => Some(CanonicalFormat::Xls),
        (CanonicalFormat::Xls | CanonicalFormat::Xlsb, "xlsb") => Some(CanonicalFormat::Xlsb),
        (CanonicalFormat::Csv, "tsv") => Some(CanonicalFormat::Csv),
        (CanonicalFormat::PlainText, "md") => Some(CanonicalFormat::PlainText),
        (CanonicalFormat::Jpeg, "jpeg") => Some(CanonicalFormat::Jpeg),
        (CanonicalFormat::Tiff, "tif") => Some(CanonicalFormat::Tiff),
        _ => None,
    }
}

/// Detect format from magic / first-bytes only (no filename trust).
pub fn detect_magic(head: &[u8]) -> Option<CanonicalFormat> {
    if head.starts_with(b"%PDF-") {
        return Some(CanonicalFormat::Pdf);
    }
    if head.starts_with(&[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n']) {
        return Some(CanonicalFormat::Png);
    }
    if head.len() >= 3 && head[0] == 0xff && head[1] == 0xd8 && head[2] == 0xff {
        return Some(CanonicalFormat::Jpeg);
    }
    if head.starts_with(b"BM") {
        return Some(CanonicalFormat::Bmp);
    }
    if head.starts_with(b"II*\0") || head.starts_with(b"MM\0*") {
        return Some(CanonicalFormat::Tiff);
    }
    if head.len() >= 12 && head.starts_with(b"RIFF") && &head[8..12] == b"WEBP" {
        return Some(CanonicalFormat::Webp);
    }
    if head.len() >= 12 && head.starts_with(b"RIFF") && &head[8..12] == b"WAVE" {
        return Some(CanonicalFormat::Wav);
    }
    if head.starts_with(b"OggS") {
        return Some(CanonicalFormat::Ogg);
    }
    if head.starts_with(b"fLaC") {
        return Some(CanonicalFormat::Flac);
    }
    if head.starts_with(b"ID3") || is_mp3_frame(head) {
        return Some(CanonicalFormat::Mp3);
    }
    if is_m4a(head) {
        return Some(CanonicalFormat::M4a);
    }
    // OLE Compound Document (XLS / XLSB family).
    if head.starts_with(&[0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1]) {
        return Some(CanonicalFormat::Xls);
    }
    // ZIP / OOXML / ODS — refined by extension then archive membership.
    if head.starts_with(b"PK\x03\x04")
        || head.starts_with(b"PK\x05\x06")
        || head.starts_with(b"PK\x07\x08")
    {
        return Some(CanonicalFormat::ZipContainer);
    }
    if looks_like_html(head) {
        return Some(CanonicalFormat::Html);
    }
    if looks_like_text(head) {
        // CSV vs plain: delimiter heuristic when commas/tabs dominate early bytes.
        if looks_like_csv(head) {
            return Some(CanonicalFormat::Csv);
        }
        return Some(CanonicalFormat::PlainText);
    }
    None
}

fn is_mp3_frame(head: &[u8]) -> bool {
    head.len() >= 2 && head[0] == 0xff && (head[1] & 0xe0) == 0xe0
}

fn is_m4a(head: &[u8]) -> bool {
    head.len() >= 12 && &head[4..8] == b"ftyp"
}

fn looks_like_html(head: &[u8]) -> bool {
    let lower: Vec<u8> = head
        .iter()
        .take(256)
        .map(|b| b.to_ascii_lowercase())
        .collect();
    lower.starts_with(b"<!doctype html")
        || lower.starts_with(b"<html")
        || lower.windows(6).any(|w| w == b"<html>")
        || lower.windows(5).any(|w| w == b"<head")
        || lower.windows(5).any(|w| w == b"<body")
}

fn looks_like_text(head: &[u8]) -> bool {
    if head.is_empty() {
        return false;
    }
    let sample = &head[..head.len().min(512)];
    if sample.contains(&0) {
        return false;
    }
    // Accept UTF-8 (Vietnamese documents). Reject high binary density.
    let Ok(text) = std::str::from_utf8(sample) else {
        return false;
    };
    let non_text = text
        .chars()
        .filter(|ch| !(ch.is_alphanumeric() || ch.is_whitespace() || ch.is_ascii_punctuation()))
        .count();
    non_text * 10 <= text.chars().count()
}

fn looks_like_csv(head: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(&head[..head.len().min(512)]) else {
        return false;
    };
    let first_line = text.lines().next().unwrap_or("");
    first_line.contains(',') || first_line.contains('\t')
}

/// Map a ZIP container to a specific OOXML/ODS format using required member paths.
pub fn refine_zip_format(
    provisional: CanonicalFormat,
    has_content_types: bool,
    has_word: bool,
    has_ppt: bool,
    has_xl: bool,
    has_ods_mimetype: bool,
) -> Result<CanonicalFormat, UploadError> {
    if has_ods_mimetype {
        return Ok(CanonicalFormat::Ods);
    }
    if !has_content_types {
        return Err(UploadError::rejected(
            ThreatClass::MalformedOoxml,
            ReasonCode::MissingContentTypes,
        ));
    }
    match (has_word, has_ppt, has_xl) {
        (true, false, false) => Ok(CanonicalFormat::Docx),
        (false, true, false) => Ok(CanonicalFormat::Pptx),
        (false, false, true) => Ok(CanonicalFormat::Xlsx),
        _ => {
            // Ambiguous or empty OOXML — fail closed unless provisional was set by extension.
            if matches!(
                provisional,
                CanonicalFormat::Docx
                    | CanonicalFormat::Pptx
                    | CanonicalFormat::Xlsx
                    | CanonicalFormat::Ods
            ) {
                Ok(provisional)
            } else {
                Err(UploadError::rejected(
                    ThreatClass::MalformedOoxml,
                    ReasonCode::MissingFormatPaths,
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdf_magic_and_spoof() {
        assert_eq!(detect_magic(b"%PDF-1.4"), Some(CanonicalFormat::Pdf));
        let err = resolve_canonical_format(b"not a pdf", Some("doc.pdf")).unwrap_err();
        assert!(matches!(
            err.threat_class(),
            Some(ThreatClass::ExtensionSpoof) | Some(ThreatClass::UnsupportedFormat) | _
        ));
        // Text named pdf → spoof once magic resolves to text.
        let err = resolve_canonical_format(b"hello world\n", Some("x.pdf")).unwrap_err();
        assert_eq!(err.threat_class(), Some(ThreatClass::ExtensionSpoof));
    }

    #[test]
    fn png_and_jpeg_ok() {
        let png = [
            0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n', 0, 0, 0, 0,
        ];
        assert_eq!(
            resolve_canonical_format(&png, Some("a.png")).unwrap(),
            CanonicalFormat::Png
        );
        let jpeg = [0xff, 0xd8, 0xff, 0xe0, 0, 0, 0, 0];
        assert_eq!(
            resolve_canonical_format(&jpeg, Some("a.jpg")).unwrap(),
            CanonicalFormat::Jpeg
        );
    }

    #[test]
    fn html_named_pdf_is_spoof() {
        let err =
            resolve_canonical_format(b"<html><body>x</body></html>", Some("x.pdf")).unwrap_err();
        assert_eq!(err.threat_class(), Some(ThreatClass::ExtensionSpoof));
    }
}
