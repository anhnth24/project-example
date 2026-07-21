//! HTML → Markdown.
//!
//! Dùng `htmd` (dựa trên html5ever) THAY cho `html2md` của markitdown-rs — sửa lỗi
//! `html2md` phình output khổng lồ (88 triệu ký tự) trên trang Wikipedia lớn.
//!
//! Decode bytes (HTML-compatible, ưu tiên nội dung đúng):
//! 1. BOM UTF-8 / UTF-16 LE / UTF-16 BE thắng meta
//! 2. Charset từ thuộc tính `<meta>` / `<?xml …?>` trong head (bỏ comment/script/style/body)
//! 3. Label chuẩn qua `encoding_rs` (gồm windows-1252 / windows-1258)
//! 4. Alias legacy VN tường minh → `viet_legacy`
//! 5. Không khai báo → `viet_legacy::decode_text` (UTF-8 hoặc heuristic có kiểm soát)

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

/// Đọc HTML bytes → Unicode string.
fn decode_html_bytes(raw: &[u8]) -> String {
    if let Some((kind, payload)) = split_bom(raw) {
        return decode_bom_payload(kind, payload);
    }

    if let Some(label) = sniff_declared_charset(raw) {
        if let Some(text) = decode_declared_charset(raw, &label) {
            return text;
        }
    }

    // Không BOM / không charset chuẩn nhận được → fallback legacy có kiểm soát.
    crate::viet_legacy::decode_text(raw)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BomKind {
    Utf8,
    Utf16Le,
    Utf16Be,
}

fn split_bom(bytes: &[u8]) -> Option<(BomKind, &[u8])> {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        Some((BomKind::Utf8, &bytes[3..]))
    } else if bytes.starts_with(&[0xFF, 0xFE]) {
        Some((BomKind::Utf16Le, &bytes[2..]))
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        Some((BomKind::Utf16Be, &bytes[2..]))
    } else {
        None
    }
}

fn decode_bom_payload(kind: BomKind, payload: &[u8]) -> String {
    match kind {
        BomKind::Utf8 => {
            let (cow, _, _) = encoding_rs::UTF_8.decode(payload);
            cow.into_owned()
        }
        BomKind::Utf16Le => {
            let (cow, _, _) = encoding_rs::UTF_16LE.decode(payload);
            cow.into_owned()
        }
        BomKind::Utf16Be => {
            let (cow, _, _) = encoding_rs::UTF_16BE.decode(payload);
            cow.into_owned()
        }
    }
}

fn decode_declared_charset(bytes: &[u8], label: &str) -> Option<String> {
    if is_tcvn3_charset(label) {
        return Some(crate::viet_legacy::decode_tcvn3(bytes));
    }
    if is_vni_charset(label) {
        return Some(crate::viet_legacy::decode_vni(bytes));
    }
    if is_vps_charset(label) {
        return Some(crate::viet_legacy::decode_vps(bytes));
    }
    let encoding = encoding_rs::Encoding::for_label(label.as_bytes())?;
    let (cow, _, _) = encoding.decode(bytes);
    Some(cow.into_owned())
}

fn is_tcvn3_charset(name: &str) -> bool {
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

/// Charset từ thuộc tính meta/XML trong phần head (không đọc body / comment / script).
fn sniff_declared_charset(bytes: &[u8]) -> Option<String> {
    let end = head_scan_limit(bytes);
    scan_head_for_charset(&bytes[..end])
}

fn head_scan_limit(bytes: &[u8]) -> usize {
    // Cắt trước `<body` nếu thấy; vẫn giới hạn để tránh quét cả file lớn.
    let cap = bytes.len().min(8192);
    let head = &bytes[..cap];
    let lower: Vec<u8> = head.iter().map(|b| b.to_ascii_lowercase()).collect();
    find_bytes(&lower, b"<body")
        .or_else(|| find_bytes(&lower, b"<frameset"))
        .unwrap_or(cap)
}

fn scan_head_for_charset(bytes: &[u8]) -> Option<String> {
    let lower: Vec<u8> = bytes.iter().map(|b| b.to_ascii_lowercase()).collect();
    let mut index = 0usize;
    while index < lower.len() {
        if lower[index] != b'<' {
            index += 1;
            continue;
        }
        // Comment
        if lower[index..].starts_with(b"<!--") {
            index += 4;
            if let Some(rel) = find_bytes(&lower[index..], b"-->") {
                index += rel + 3;
            } else {
                break;
            }
            continue;
        }
        // DOCTYPE / other declarations that are not xml
        if lower[index..].starts_with(b"<!") {
            index = skip_until(&lower, index + 2, b'>');
            continue;
        }
        // XML declaration
        if lower[index..].starts_with(b"<?xml") {
            let close = find_bytes(&lower[index..], b"?>").map(|r| index + r)?;
            let attrs = parse_tag_attributes(&lower[index + 5..close]);
            if let Some(value) = attr_value(&attrs, "encoding") {
                return Some(value);
            }
            index = close + 2;
            continue;
        }
        // End of head — stop before body content markup we already cut, but also </head>
        if lower[index..].starts_with(b"</head") {
            break;
        }
        // Skip script / style entirely (false-positive charset= in JS/CSS).
        if let Some(end_tag) = match_open_skip_element(&lower[index..]) {
            let open_end = skip_until(&lower, index + 1, b'>');
            if let Some(rel) = find_bytes(&lower[open_end..], end_tag) {
                index = open_end + rel + end_tag.len();
            } else {
                break;
            }
            continue;
        }
        // Meta
        if lower[index..].starts_with(b"<meta")
            && matches!(
                lower.get(index + 5).copied().unwrap_or(0),
                b'>' | b'/' | b'\t' | b'\n' | b'\r' | b' '
            )
        {
            let close = skip_until(&lower, index + 5, b'>');
            let attrs = parse_tag_attributes(&lower[index + 5..close]);
            if let Some(value) = charset_from_meta_attrs(&attrs) {
                return Some(value);
            }
            index = if close < lower.len() {
                close + 1
            } else {
                close
            };
            continue;
        }
        // Other tag — skip.
        index = skip_until(&lower, index + 1, b'>');
        if index < lower.len() {
            index += 1;
        }
    }
    None
}

fn match_open_skip_element(lower_at_lt: &[u8]) -> Option<&'static [u8]> {
    for (open, close) in [
        (&b"<script"[..], &b"</script>"[..]),
        (&b"<style"[..], &b"</style>"[..]),
        (&b"<noscript"[..], &b"</noscript>"[..]),
    ] {
        if lower_at_lt.starts_with(open) {
            let next = lower_at_lt.get(open.len()).copied().unwrap_or(0);
            if matches!(next, b'>' | b'/' | b'\t' | b'\n' | b'\r' | b' ') {
                return Some(close);
            }
        }
    }
    None
}

fn charset_from_meta_attrs(attrs: &[(String, String)]) -> Option<String> {
    if let Some(value) = attr_value(attrs, "charset") {
        return Some(value);
    }
    let http_equiv = attr_value(attrs, "http-equiv")?;
    if http_equiv != "content-type" {
        return None;
    }
    let content = attr_value(attrs, "content")?;
    charset_from_content_type(&content)
}

fn charset_from_content_type(content: &str) -> Option<String> {
    let lower = content.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut start = 0usize;
    while let Some(rel) = find_bytes(&bytes[start..], b"charset") {
        let at = start + rel + b"charset".len();
        let mut i = at;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'=' {
            start = at;
            continue;
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        return read_charset_token(&bytes[i..]);
    }
    None
}

fn attr_value(attrs: &[(String, String)], name: &str) -> Option<String> {
    attrs
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
}

/// Parse `name = value` pairs; cho phép khoảng trắng quanh `=`.
fn parse_tag_attributes(bytes: &[u8]) -> Vec<(String, String)> {
    let mut attrs = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        while index < bytes.len()
            && (bytes[index].is_ascii_whitespace() || bytes[index] == b'/' || bytes[index] == b'?')
        {
            index += 1;
        }
        if index >= bytes.len() || bytes[index] == b'>' {
            break;
        }
        let name_start = index;
        while index < bytes.len()
            && !bytes[index].is_ascii_whitespace()
            && bytes[index] != b'='
            && bytes[index] != b'/'
            && bytes[index] != b'>'
        {
            index += 1;
        }
        if name_start == index {
            index += 1;
            continue;
        }
        let name = std::str::from_utf8(&bytes[name_start..index])
            .unwrap_or("")
            .to_ascii_lowercase();
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= bytes.len() || bytes[index] != b'=' {
            // Boolean attribute — ignore for charset purposes.
            continue;
        }
        index += 1;
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        let Some(value) = read_attr_value(&bytes[index..]) else {
            break;
        };
        index += value.consumed;
        if !name.is_empty() {
            attrs.push((name, value.text.to_ascii_lowercase()));
        }
    }
    attrs
}

struct AttrValue {
    text: String,
    consumed: usize,
}

fn read_attr_value(bytes: &[u8]) -> Option<AttrValue> {
    if bytes.is_empty() {
        return None;
    }
    if bytes[0] == b'"' || bytes[0] == b'\'' {
        let quote = bytes[0];
        let end = bytes[1..].iter().position(|&b| b == quote)? + 1;
        let text = std::str::from_utf8(&bytes[1..end]).ok()?.to_string();
        return Some(AttrValue {
            text,
            consumed: end + 1,
        });
    }
    let end = bytes
        .iter()
        .position(|&b| b.is_ascii_whitespace() || b == b'>' || b == b'/' || b == b'?')
        .unwrap_or(bytes.len());
    let text = std::str::from_utf8(&bytes[..end]).ok()?.to_string();
    Some(AttrValue {
        text,
        consumed: end,
    })
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
                    b.is_ascii_whitespace() || matches!(b, b'"' | b'\'' | b';' | b'>' | b'/')
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

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn skip_until(bytes: &[u8], start: usize, marker: u8) -> usize {
    bytes[start..]
        .iter()
        .position(|&b| b == marker)
        .map(|p| start + p)
        .unwrap_or(bytes.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Converter;
    use encoding_rs::WINDOWS_1258;

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
    fn utf8_bom_wins_over_conflicting_meta() {
        // BOM UTF-8 + meta windows-1252: BOM thắng (HTML-compatible).
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(
            b"<html><head><meta charset=\"windows-1252\"></head><body><p>Xin ch\xc3\xa0o</p></body></html>",
        );
        let decoded = decode_html_bytes(&bytes);
        assert!(decoded.contains("Xin chào"), "got: {decoded:?}");
        assert!(
            !decoded.contains("Ã"),
            "must not decode as 1252: {decoded:?}"
        );
    }

    #[test]
    fn utf16_le_bom_decodes_vietnamese() {
        let text = "<html><body><p>Xin chào</p></body></html>";
        let mut bytes = vec![0xFF, 0xFE];
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        let path = temp_html("utf16le", &bytes);
        let md = Converter::new().convert_path(&path).unwrap().markdown;
        assert!(md.contains("Xin chào"), "got: {md:?}");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn meta_charset_whitespace_around_equals() {
        let html = b"<html><head><meta charset = \"utf-8\"></head><body><p>ok</p></body></html>";
        assert_eq!(sniff_declared_charset(html).as_deref(), Some("utf-8"));
        let http = b"<html><head><meta http-equiv = \"Content-Type\" content = \"text/html; charset = windows-1258\"></head><body></body></html>";
        assert_eq!(
            sniff_declared_charset(http).as_deref(),
            Some("windows-1258")
        );
    }

    #[test]
    fn ignores_charset_in_comments_scripts_and_body() {
        let html = b"\
<html><head>\
<!-- <meta charset=\"windows-1252\"> -->\
<script>var x = 'charset=iso-8859-1';</script>\
<meta charset=\"utf-8\">\
</head><body>charset=tcvn3 fake</body></html>";
        assert_eq!(sniff_declared_charset(html).as_deref(), Some("utf-8"));
    }

    #[test]
    fn body_charset_text_is_not_a_declaration() {
        let html = b"<html><body><p>charset=windows-1252</p></body></html>";
        assert_eq!(sniff_declared_charset(html), None);
    }

    #[test]
    fn windows_1258_meta_decodes_vietnamese() {
        // "Xin chào" in windows-1258: à = 0xE0 (same as Latin-1 for this word).
        let (encoded, _, _) = WINDOWS_1258.encode("Xin chào");
        let mut html = b"<html><head><meta charset=\"windows-1258\"></head><body><p>".to_vec();
        html.extend_from_slice(&encoded);
        html.extend_from_slice(b"</p></body></html>");
        let path = temp_html("cp1258", &html);
        let md = Converter::new().convert_path(&path).unwrap().markdown;
        assert!(md.contains("Xin chào"), "got: {md:?}");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn windows_1252_meta_decodes_latin() {
        let html =
            b"<html><head><meta charset=\"windows-1252\"></head><body><p>caf\xe9</p></body></html>";
        let decoded = decode_html_bytes(html);
        assert!(decoded.contains("café"), "got: {decoded:?}");
    }

    #[test]
    fn meta_charset_tcvn3_decodes_legacy_body() {
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
    fn heuristic_legacy_without_meta_still_decodes() {
        let mut html = b"<html><body><p>".to_vec();
        html.extend_from_slice(&[
            0x43, 0xE9, 0x6E, 0x67, 0x20, 0x68, 0xDF, 0x61, 0x20, 0x78, 0xB7,
        ]);
        html.extend_from_slice(b"</p></body></html>");
        let decoded = decode_html_bytes(&html);
        assert!(decoded.contains("Cộng hòa xã"), "got: {decoded:?}");
    }

    #[test]
    fn xml_declaration_encoding_is_honoured() {
        let html =
            br#"<?xml version="1.0" encoding = "utf-8"?><html><body><p>hi</p></body></html>"#;
        assert_eq!(sniff_declared_charset(html).as_deref(), Some("utf-8"));
    }
}
