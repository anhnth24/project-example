//! Trích BẢNG có cấu trúc → JSON (tất định, KHÔNG cần LLM). Cho xlsx/xls/csv.
//!
//! Trả JSON: xlsx → `{ "<sheet>": [[ô,...], ...], ... }`; csv → `[[ô,...], ...]`.
//! Chuỗi text trong JSON được chuẩn hoá NFC (cùng hợp đồng với `convert_path`).

use std::path::Path;

use unicode_normalization::{is_nfc_quick, IsNormalized, UnicodeNormalization};

use crate::{ConvertError, FormatKind};

pub fn tables_json(path: &Path, sheet: Option<&str>) -> Result<String, ConvertError> {
    match FormatKind::from_path(path) {
        FormatKind::Xlsx => xlsx_json(path, sheet),
        FormatKind::Csv => csv_json(path),
        other => Err(ConvertError::Unsupported(match other {
            FormatKind::Pdf => "extract_tables_json: PDF chưa hỗ trợ (dùng convert_to_markdown)",
            _ => "extract_tables_json: chỉ hỗ trợ xlsx/xls/csv",
        })),
    }
}

fn normalize_nfc(text: String) -> String {
    match is_nfc_quick(text.chars()) {
        IsNormalized::Yes => text,
        _ => text.nfc().collect(),
    }
}

fn json_string(text: impl Into<String>) -> serde_json::Value {
    serde_json::Value::String(normalize_nfc(text.into()))
}

fn xlsx_json(path: &Path, sheet: Option<&str>) -> Result<String, ConvertError> {
    use calamine::{open_workbook_auto, Data, Reader};
    let mut wb = open_workbook_auto(path).map_err(|e| ConvertError::Failed(e.to_string()))?;
    let names: Vec<String> = match sheet {
        Some(want) => wb
            .sheet_names()
            .iter()
            .filter(|n| n.eq_ignore_ascii_case(want))
            .cloned()
            .collect(),
        None => wb.sheet_names().to_owned(),
    };

    let mut obj = serde_json::Map::new();
    for name in names {
        let Ok(range) = wb.worksheet_range(&name) else {
            continue;
        };
        let rows: Vec<serde_json::Value> = range
            .rows()
            .map(|row| {
                serde_json::Value::Array(
                    row.iter()
                        .map(|c| match c {
                            Data::Empty => serde_json::Value::Null,
                            Data::Int(i) => (*i).into(),
                            Data::Float(f) => (*f).into(),
                            Data::Bool(b) => (*b).into(),
                            other => json_string(other.to_string()),
                        })
                        .collect(),
                )
            })
            .collect();
        obj.insert(normalize_nfc(name), serde_json::Value::Array(rows));
    }
    serde_json::to_string(&serde_json::Value::Object(obj))
        .map_err(|e| ConvertError::Failed(e.to_string()))
}

fn csv_json(path: &Path) -> Result<String, ConvertError> {
    let raw = std::fs::read(path).map_err(|e| ConvertError::Failed(e.to_string()))?;
    let bytes = raw.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&raw[..]);
    let text = crate::viet_legacy::decode_text(bytes);
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(text.as_bytes());
    let rows: Vec<serde_json::Value> = rdr
        .records()
        .filter_map(|r| r.ok())
        .map(|rec| serde_json::Value::Array(rec.iter().map(|s| json_string(s)).collect()))
        .collect();
    serde_json::to_string(&serde_json::Value::Array(rows))
        .map_err(|e| ConvertError::Failed(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_csv_decodes_tcvn3_like_markdown_converter() {
        let path =
            std::env::temp_dir().join(format!("fileconv_tables_tcvn3_{}.csv", std::process::id()));
        let bytes = [
            0x43, 0xE9, 0x6E, 0x67, 0x20, 0x68, 0xDF, 0x61, 0x20, 0x78, 0xB7, 0x20, 0x68, 0xE9,
            0x69,
        ];
        std::fs::write(&path, bytes).unwrap();
        let json = tables_json(&path, None).unwrap();
        assert!(json.contains("Cộng hòa xã hội"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn csv_json_normalizes_nfd_cells_to_nfc() {
        // "tiếng" NFD: e + ◌̂ + ◌́
        let nfd = "ti\u{0065}\u{0302}\u{0301}ng,Vi\u{0065}\u{0323}\u{0302}t\n";
        let path =
            std::env::temp_dir().join(format!("fileconv_tables_nfd_{}.csv", std::process::id()));
        std::fs::write(&path, nfd).unwrap();
        let json = tables_json(&path, None).unwrap();
        assert!(json.contains("tiếng"), "got: {json}");
        assert!(json.contains("Việt"), "got: {json}");
        assert!(
            !json.chars().any(|c| ('\u{0300}'..='\u{036F}').contains(&c)),
            "JSON còn combining mark: {json}"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn csv_json_decodes_tcvn3_uppercase_digraph_cell() {
        // "Á,ĐỘ" — hoa có dấu TCVN3.
        let bytes = [0x41, 0xB8, b',', 0xA7, 0xA4, 0xE9, b'\n'];
        let path = std::env::temp_dir().join(format!(
            "fileconv_tables_tcvn3_upper_{}.csv",
            std::process::id()
        ));
        std::fs::write(&path, bytes).unwrap();
        let json = tables_json(&path, None).unwrap();
        assert!(json.contains("Á"), "got: {json}");
        assert!(json.contains("ĐỘ"), "got: {json}");
        let _ = std::fs::remove_file(path);
    }
}
