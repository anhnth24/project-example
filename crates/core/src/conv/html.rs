//! HTML → Markdown.
//!
//! Dùng `htmd` (dựa trên html5ever) THAY cho `html2md` của markitdown-rs — sửa lỗi
//! `html2md` phình output khổng lồ (88 triệu ký tự) trên trang Wikipedia lớn.
//!
//! Decode bytes trước khi parse: BOM UTF-8, `meta charset` / XML encoding, và
//! bảng mã VN cũ qua `viet_legacy` (ưu tiên nội dung đúng hơn format).

use std::path::Path;

use super::fail;
use crate::ConvertError;

pub fn to_markdown(path: &Path) -> Result<String, ConvertError> {
    let bytes = std::fs::read(path).map_err(fail)?;
    let html = decode_html_bytes(&bytes);
    // Bỏ <script>/<style> để không lọt mã JS/CSS vào Markdown.
    let converter = htmd::HtmlToMarkdown::builder()
        .skip_tags(vec!["script", "style", "noscript"])
        .build();
    converter.convert(&html).map_err(fail)
}

/// Đọc HTML bytes → Unicode string (BOM + meta charset + legacy VN).
fn decode_html_bytes(raw: &[u8]) -> String {
    let bytes = strip_utf8_bom(raw);
    match sniff_meta_charset(bytes).as_deref() {
        Some(cs) if is_utf8_charset(cs) => decode_utf8_or_legacy(bytes),
        Some(cs) if is_tcvn3_charset(cs) => crate::viet_legacy::decode_tcvn3(bytes),
        Some(cs) if is_vni_charset(cs) => crate::viet_legacy::decode_vni(bytes),
        Some(cs) if is_vps_charset(cs) => crate::viet_legacy::decode_vps(bytes),
        // Charset lạ / thiếu: decode_text (UTF-8 hoặc heuristic TCVN3/VNI/VPS).
        _ => crate::viet_legacy::decode_text(bytes),
    }
}

fn strip_utf8_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes)
}

fn decode_utf8_or_legacy(bytes: &[u8]) -> String {
    // Meta khai UTF-8 nhưng payload có thể là TCVN3 giả UTF-8 — ưu tiên nội dung.
    crate::viet_legacy::decode_text(bytes)
}

fn is_utf8_charset(name: &str) -> bool {
    matches!(name, "utf-8" | "utf8" | "utf_8")
}

fn is_tcvn3_charset(name: &str) -> bool {
    // Chỉ alias tường minh — tránh `vietnamese` (hay trỏ windows-1258/VISCII).
    matches!(
        name,
        "tcvn3" | "tcvn-3" | "tcvn_3" | "tcvn" | "x-tcvn3" | "x-tcvn"
    )
}

fn is_vni_charset(name: &str) -> bool {
    matches!(name, "vni" | "vni-windows" | "vni_windows" | "x-vni")
}

fn is_vps_charset(name: &str) -> bool {
    matches!(name, "vps" | "x-vps")
}

/// Quét đầu file (ASCII-compatible) lấy charset từ `<meta charset>`,
/// `http-equiv=Content-Type`, hoặc `<?xml ... encoding=...>`.
fn sniff_meta_charset(bytes: &[u8]) -> Option<String> {
    let head = &bytes[..bytes.len().min(4096)];
    let lower: Vec<u8> = head.iter().map(|b| b.to_ascii_lowercase()).collect();

    if let Some(value) = charset_after_key(&lower, b"charset=") {
        return Some(value);
    }
    if let Some(value) = charset_after_key(&lower, b"encoding=") {
        return Some(value);
    }
    None
}

fn charset_after_key(haystack: &[u8], key: &[u8]) -> Option<String> {
    let mut start = 0usize;
    while let Some(rel) = find_bytes(&haystack[start..], key) {
        let value_at = start + rel + key.len();
        if let Some(value) = read_charset_token(&haystack[value_at..]) {
            return Some(value);
        }
        start = value_at;
    }
    None
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn read_charset_token(bytes: &[u8]) -> Option<String> {
    let mut index = 0usize;
    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    if index >= bytes.len() {
        return None;
    }
    let token = match bytes[index] {
        b'"' | b'\'' => {
            let quote = bytes[index];
            index += 1;
            let end = bytes[index..]
                .iter()
                .position(|&b| b == quote)
                .map(|p| index + p)?;
            &bytes[index..end]
        }
        _ => {
            let end = bytes[index..]
                .iter()
                .position(|&b| {
                    matches!(
                        b,
                        b'"' | b'\'' | b';' | b'>' | b' ' | b'\t' | b'\n' | b'\r' | b'/'
                    )
                })
                .map(|p| index + p)
                .unwrap_or(bytes.len());
            &bytes[index..end]
        }
    };
    if token.is_empty() || !token.iter().all(|b| b.is_ascii()) {
        return None;
    }
    let name = std::str::from_utf8(token).ok()?.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_ascii_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Converter;

    fn temp_html(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "fileconv_html_{}_{}.html",
            name,
            std::process::id()
        ));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn strips_utf8_bom_before_convert() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(
            b"<html><head><meta charset=\"utf-8\"></head><body><p>Xin ch\xc3\xa0o</p></body></html>",
        );
        let path = temp_html("bom", &bytes);
        let md = Converter::new().convert_path(&path).unwrap().markdown;
        assert!(md.contains("Xin chào"), "got: {md:?}");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn meta_charset_tcvn3_decodes_legacy_body() {
        // "Cộng hòa" lowercase TCVN3 + meta khai báo tcvn3 (ASCII).
        let mut html =
            b"<!DOCTYPE html><html><head><meta charset=\"tcvn3\"></head><body><p>".to_vec();
        html.extend_from_slice(&[0x43, 0xE9, 0x6E, 0x67, 0x20, 0x68, 0xDF, 0x61]);
        html.extend_from_slice(b"</p></body></html>");
        let path = temp_html("tcvn3_meta", &html);
        let md = Converter::new().convert_path(&path).unwrap().markdown;
        assert!(md.contains("Cộng hòa"), "got: {md:?}");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn meta_charset_tcvn3_uppercase_digraphs() {
        // "Á ĐỘ" — hoa có dấu qua digraph VietUnicode.
        let mut html =
            b"<html><head><meta http-equiv=\"Content-Type\" content=\"text/html; charset=TCVN3\"></head><body><h1>"
                .to_vec();
        html.extend_from_slice(&[0x41, 0xB8, 0x20, 0xA7, 0xA4, 0xE9]);
        html.extend_from_slice(b"</h1></body></html>");
        assert_eq!(sniff_meta_charset(&html).as_deref(), Some("tcvn3"));
        let path = temp_html("tcvn3_upper", &html);
        let md = Converter::new().convert_path(&path).unwrap().markdown;
        assert!(md.contains("Á ĐỘ"), "got: {md:?}");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn heuristic_legacy_without_meta_still_decodes() {
        let mut html = b"<html><body><p>".to_vec();
        html.extend_from_slice(&[
            0x43, 0xE9, 0x6E, 0x67, 0x20, 0x68, 0xDF, 0x61, 0x20, 0x78, 0xB7,
        ]);
        html.extend_from_slice(b"</p></body></html>");
        let decoded = decode_html_bytes(&html);
        assert!(decoded.contains("Cộng hòa xã"), "got: {decoded:?}");
    }
}
