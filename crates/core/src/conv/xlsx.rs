//! Excel (xlsx/xls/xlsb/ods) → Markdown. Sửa lỗi markitdown-rs: đọc TẤT CẢ sheet
//! (không chỉ sheet đầu) và hỗ trợ cả định dạng cũ `.xls` (calamine open_workbook_auto).

use std::path::Path;

use calamine::{open_workbook_auto, Data, Dimensions, Reader, Sheets};

use super::{esc_cell, fail};
use crate::conv::csv_conv::{rows_to_html_table, rows_to_md_table, MergeRange};
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
        let merge_dimensions = worksheet_merges(&mut wb, &name);
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
        let origin = range.start().unwrap_or((0, 0));
        let merges: Vec<MergeRange> = merge_dimensions
            .into_iter()
            .filter_map(|dimensions| relative_merge(dimensions, origin))
            .collect();
        if !merges.is_empty()
            || rows
                .iter()
                .flatten()
                .any(|cell| cell.contains('\n') || cell.contains('\r'))
        {
            out.push_str(&rows_to_html_table(&rows, &merges));
        } else {
            let escaped: Vec<Vec<String>> = rows
                .iter()
                .map(|row| row.iter().map(|cell| esc_cell(cell)).collect())
                .collect();
            out.push_str(&rows_to_md_table(&escaped));
        }
        out.push('\n');
    }
    Ok(out)
}

fn cell_to_string(c: &Data) -> String {
    match c {
        Data::Empty => String::new(),
        other => other.to_string(),
    }
}

fn worksheet_merges(
    workbook: &mut Sheets<std::io::BufReader<std::fs::File>>,
    name: &str,
) -> Vec<Dimensions> {
    match workbook {
        Sheets::Xls(reader) => reader.worksheet_merge_cells(name).unwrap_or_default(),
        Sheets::Xlsx(reader) => reader
            .worksheet_merge_cells(name)
            .and_then(Result::ok)
            .unwrap_or_default(),
        Sheets::Xlsb(_) | Sheets::Ods(_) => Vec::new(),
    }
}

fn relative_merge(dimensions: Dimensions, origin: (u32, u32)) -> Option<MergeRange> {
    if dimensions.start.0 < origin.0 || dimensions.start.1 < origin.1 {
        return None;
    }
    Some(MergeRange {
        start_row: (dimensions.start.0 - origin.0) as usize,
        start_col: (dimensions.start.1 - origin.1) as usize,
        end_row: (dimensions.end.0 - origin.0) as usize,
        end_col: (dimensions.end.1 - origin.1) as usize,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_dimensions_are_relative_to_nonempty_range_origin() {
        let merge = relative_merge(
            Dimensions {
                start: (3, 4),
                end: (5, 6),
            },
            (2, 3),
        )
        .unwrap();
        assert_eq!(
            merge,
            MergeRange {
                start_row: 1,
                start_col: 1,
                end_row: 3,
                end_col: 3,
            }
        );
    }
}
