//! Trích BẢNG có cấu trúc → JSON (tất định, KHÔNG cần LLM). Cho xlsx/xls/csv.
//!
//! Trả JSON: xlsx → `{ "<sheet>": [[ô,...], ...], ... }`; csv → `[[ô,...], ...]`.

use std::path::Path;

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
                            other => serde_json::Value::String(other.to_string()),
                        })
                        .collect(),
                )
            })
            .collect();
        obj.insert(name, serde_json::Value::Array(rows));
    }
    serde_json::to_string(&serde_json::Value::Object(obj))
        .map_err(|e| ConvertError::Failed(e.to_string()))
}

fn csv_json(path: &Path) -> Result<String, ConvertError> {
    let raw = std::fs::read(path).map_err(|e| ConvertError::Failed(e.to_string()))?;
    let bytes = raw.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&raw[..]);
    let text = String::from_utf8_lossy(bytes);
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(text.as_bytes());
    let rows: Vec<serde_json::Value> = rdr
        .records()
        .filter_map(|r| r.ok())
        .map(|rec| {
            serde_json::Value::Array(
                rec.iter()
                    .map(|s| serde_json::Value::String(s.to_string()))
                    .collect(),
            )
        })
        .collect();
    serde_json::to_string(&serde_json::Value::Array(rows))
        .map_err(|e| ConvertError::Failed(e.to_string()))
}
