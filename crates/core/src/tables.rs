//! Trích BẢNG có cấu trúc → JSON (tất định, KHÔNG cần LLM). Cho xlsx/xls/csv.
//!
//! Trả JSON: xlsx → `{ "<sheet>": [[ô,...], ...], ... }`; csv → `[[ô,...], ...]`.
//! Chuỗi text trong JSON được chuẩn hoá NFC (cùng hợp đồng với `convert_path`).
//! Tên sheet sau NFC không được ghi đè thầm — trùng key → lỗi.

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

/// Chèn sheet vào map JSON; lỗi nếu hai tên (khác nhau trước NFC) trùng key sau NFC.
fn insert_sheet_rows(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    name: String,
    rows: serde_json::Value,
) -> Result<(), ConvertError> {
    let key = normalize_nfc(name.clone());
    if obj.contains_key(&key) {
        return Err(ConvertError::Failed(format!(
            "extract_tables_json: tên sheet trùng sau NFC: {name:?} → {key:?}"
        )));
    }
    obj.insert(key, rows);
    Ok(())
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
        insert_sheet_rows(&mut obj, name, serde_json::Value::Array(rows))?;
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
        .map(|rec| serde_json::Value::Array(rec.iter().map(json_string).collect()))
        .collect();
    serde_json::to_string(&serde_json::Value::Array(rows))
        .map_err(|e| ConvertError::Failed(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

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
    fn sheet_name_nfc_collision_returns_error() {
        let mut obj = serde_json::Map::new();
        let nfc = "ế".to_string();
        let nfd = "e\u{0302}\u{0301}".to_string();
        assert_ne!(nfc.as_str(), nfd.as_str());
        assert_eq!(normalize_nfc(nfc.clone()), normalize_nfc(nfd.clone()));
        insert_sheet_rows(&mut obj, nfc, serde_json::json!([["a"]])).unwrap();
        let err = insert_sheet_rows(&mut obj, nfd, serde_json::json!([["b"]])).unwrap_err();
        assert!(err.to_string().contains("trùng sau NFC"), "got: {err}");
    }

    #[test]
    fn xlsx_json_normalizes_nfd_cells_and_sheet_names() {
        let nfd_sheet = "ti\u{0065}\u{0302}\u{0301}ng";
        let nfd_cell = "Vi\u{0065}\u{0323}\u{0302}t";
        let path =
            std::env::temp_dir().join(format!("fileconv_tables_nfd_{}.xlsx", std::process::id()));
        write_minimal_xlsx(&path, &[(nfd_sheet, nfd_cell)]).unwrap();
        let json = tables_json(&path, None).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let obj = value.as_object().expect("object");
        assert!(obj.contains_key("tiếng"), "keys: {:?}", obj.keys());
        let cell = obj["tiếng"][0][0].as_str().unwrap();
        assert_eq!(cell, "Việt");
        assert!(!json.chars().any(|c| ('\u{0300}'..='\u{036F}').contains(&c)));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn xlsx_json_errors_on_nfc_colliding_sheet_names() {
        let nfc = "ế";
        let nfd = "e\u{0302}\u{0301}";
        let path = std::env::temp_dir().join(format!(
            "fileconv_tables_collide_{}.xlsx",
            std::process::id()
        ));
        write_minimal_xlsx(&path, &[(nfc, "a"), (nfd, "b")]).unwrap();
        let err = tables_json(&path, None).unwrap_err();
        assert!(err.to_string().contains("trùng sau NFC"), "got: {err}");
        let _ = std::fs::remove_file(path);
    }

    /// Minimal OOXML workbook for calamine (shared strings + one/more sheets).
    fn write_minimal_xlsx(
        path: &Path,
        sheets: &[(&str, &str)],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let file = std::fs::File::create(path)?;
        let mut zip = ZipWriter::new(file);
        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

        zip.start_file("[Content_Types].xml", opts)?;
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
  <Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/>
"#,
        )?;
        for index in 1..=sheets.len() {
            writeln!(
                zip,
                r#"  <Override PartName="/xl/worksheets/sheet{index}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#
            )?;
        }
        zip.write_all(b"</Types>")?;

        zip.start_file("_rels/.rels", opts)?;
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>"#,
        )?;

        zip.start_file("xl/workbook.xml", opts)?;
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <sheets>
"#,
        )?;
        for (index, (name, _)) in sheets.iter().enumerate() {
            let escaped = xml_escape(name);
            writeln!(
                zip,
                r#"    <sheet name="{escaped}" sheetId="{id}" r:id="rId{id}"/>"#,
                id = index + 1
            )?;
        }
        zip.write_all(b"  </sheets>\n</workbook>")?;

        zip.start_file("xl/_rels/workbook.xml.rels", opts)?;
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
"#,
        )?;
        for index in 1..=sheets.len() {
            writeln!(
                zip,
                r#"  <Relationship Id="rId{index}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{index}.xml"/>"#
            )?;
        }
        write!(
            zip,
            r#"  <Relationship Id="rId{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/>
</Relationships>"#,
            sheets.len() + 1
        )?;

        // Shared strings: one entry per sheet cell value (index = sheet order).
        zip.start_file("xl/sharedStrings.xml", opts)?;
        write!(
            zip,
            r#"<?xml version="1.0" encoding="UTF-8"?>
<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="{n}" uniqueCount="{n}">
"#,
            n = sheets.len()
        )?;
        for (_, cell) in sheets {
            writeln!(
                zip,
                r#"  <si><t xml:space="preserve">{}</t></si>"#,
                xml_escape(cell)
            )?;
        }
        zip.write_all(b"</sst>")?;

        for (index, _) in sheets.iter().enumerate() {
            let sheet_path = format!("xl/worksheets/sheet{}.xml", index + 1);
            zip.start_file(&sheet_path, opts)?;
            write!(
                zip,
                r#"<?xml version="1.0" encoding="UTF-8"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <sheetData>
    <row r="1">
      <c r="A1" t="s"><v>{index}</v></c>
    </row>
  </sheetData>
</worksheet>"#
            )?;
        }

        zip.finish()?;
        Ok(())
    }

    fn xml_escape(value: &str) -> String {
        value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }
}
