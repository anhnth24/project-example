//! Xem nhanh metadata file mà KHÔNG convert — để agent/MCP quyết trích phần nào
//! (đỡ tốn token). Rẻ: pdf chỉ detect (không dựng markdown), xlsx chỉ đọc tên sheet.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

use crate::FormatKind;

/// Thông tin tóm tắt về một file.
#[derive(Debug, Clone)]
pub struct FileInfo {
    pub format: FormatKind,
    pub bytes: u64,
    /// Số trang (pdf) hoặc số slide (pptx).
    pub pages: Option<u32>,
    /// Danh sách tên sheet (xlsx/xls).
    pub sheets: Option<Vec<String>>,
}

pub fn probe(path: &Path) -> FileInfo {
    let format = FormatKind::from_path(path);
    let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let (mut pages, mut sheets) = (None, None);

    match format {
        FormatKind::Pdf => {
            if let Ok(data) = std::fs::read(path) {
                pages = catch_unwind(AssertUnwindSafe(|| {
                    pdf_inspector::detect_pdf_mem(&data).ok().map(|r| r.page_count)
                }))
                .ok()
                .flatten();
            }
        }
        FormatKind::Pptx => pages = count_pptx_slides(path),
        FormatKind::Xlsx => sheets = xlsx_sheet_names(path),
        _ => {}
    }

    FileInfo {
        format,
        bytes,
        pages,
        sheets,
    }
}

fn count_pptx_slides(path: &Path) -> Option<u32> {
    let file = std::fs::File::open(path).ok()?;
    let zip = zip::ZipArchive::new(file).ok()?;
    let n = zip
        .file_names()
        .filter(|n| {
            n.strip_prefix("ppt/slides/slide")
                .and_then(|s| s.strip_suffix(".xml"))
                .map(|num| !num.is_empty() && num.bytes().all(|b| b.is_ascii_digit()))
                .unwrap_or(false)
        })
        .count();
    Some(n as u32)
}

fn xlsx_sheet_names(path: &Path) -> Option<Vec<String>> {
    use calamine::Reader;
    let wb = calamine::open_workbook_auto(path).ok()?;
    Some(wb.sheet_names().to_owned())
}
