//! Excel (xlsx/xls/xlsb/ods) → Markdown. Sửa lỗi markitdown-rs: đọc TẤT CẢ sheet
//! (không chỉ sheet đầu) và hỗ trợ cả định dạng cũ `.xls` (calamine open_workbook_auto).

use std::path::Path;

use calamine::{open_workbook_auto, Data, Reader};

use super::{esc_cell, fail};
use crate::conv::csv_conv::rows_to_md_table;
use crate::ConvertError;

pub fn to_markdown(path: &Path, sheet: Option<&str>) -> Result<String, ConvertError> {
    let mut wb = open_workbook_auto(path).map_err(fail)?;
    let names: Vec<String> = match sheet {
        // Chỉ sheet được chọn (khớp tên không phân biệt hoa/thường).
        Some(want) => wb
            .sheet_names()
            .iter()
            .filter(|n| n.eq_ignore_ascii_case(want))
            .cloned()
            .collect(),
        None => wb.sheet_names().to_owned(),
    };
    let mut out = String::new();

    for name in names {
        let range = match wb.worksheet_range(&name) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if range.is_empty() {
            continue;
        }
        out.push_str(&format!("## {}\n\n", name));
        let rows: Vec<Vec<String>> = range
            .rows()
            .map(|row| row.iter().map(cell_to_string).collect())
            .collect();
        out.push_str(&rows_to_md_table(&rows));
        out.push('\n');
    }
    Ok(out)
}

fn cell_to_string(c: &Data) -> String {
    match c {
        Data::Empty => String::new(),
        other => esc_cell(&other.to_string()),
    }
}
