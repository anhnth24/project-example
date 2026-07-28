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
                    pdf_inspector::detect_pdf_mem(&data)
                        .ok()
                        .map(|r| r.page_count)
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

#[cfg(test)]
mod tests {
    use super::count_pptx_slides;
    use std::io::Write;
    use std::path::{Path, PathBuf};

    /// Fixture thật trong repo (`bench/markhand_web/golden/documents`, không
    /// phải `vendor/markitdown-rs`), đường dẫn giải theo `CARGO_MANIFEST_DIR`
    /// để chạy được trong CI/worktree bất kể cwd — cùng quy ước với
    /// `crates/core/tests/office_golden_e2e.rs`.
    fn golden_document(name: &str) -> PathBuf {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("bench/markhand_web/golden/documents")
            .join(name);
        assert!(
            path.is_file(),
            "missing golden fixture {name} at {}",
            path.display()
        );
        path
    }

    #[test]
    fn counts_single_slide_in_real_golden_pptx() {
        // gold-009.pptx / gold-010.pptx đều có đúng 1 slide thật.
        assert_eq!(
            count_pptx_slides(&golden_document("gold-009.pptx")),
            Some(1)
        );
        assert_eq!(
            count_pptx_slides(&golden_document("gold-010.pptx")),
            Some(1)
        );
    }

    /// Tạo một .pptx tối thiểu trong bộ nhớ: N slide thật cộng thêm các entry
    /// "nhiễu" (slideLayout, notesSlide, slide*.xml.rels) để xác nhận hàm đếm
    /// chỉ khớp `ppt/slides/slideN.xml`, không đếm nhầm layout/notes/rels —
    /// đúng những gì logic `python3` shell-out cũ (regex
    /// `ppt/slides/slide[0-9]+\.xml$`) cũng loại trừ.
    fn build_pptx(slide_count: u32) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            for i in 1..=slide_count {
                zip.start_file(format!("ppt/slides/slide{i}.xml"), opts)
                    .unwrap();
                zip.write_all(b"<p:sld/>").unwrap();
                zip.start_file(format!("ppt/slides/_rels/slide{i}.xml.rels"), opts)
                    .unwrap();
                zip.write_all(b"<Relationships/>").unwrap();
                zip.start_file(format!("ppt/notesSlides/notesSlide{i}.xml"), opts)
                    .unwrap();
                zip.write_all(b"<p:notes/>").unwrap();
            }
            zip.start_file("ppt/slideLayouts/slideLayout1.xml", opts)
                .unwrap();
            zip.write_all(b"<p:sldLayout/>").unwrap();
            zip.finish().unwrap();
        }
        buf
    }

    #[test]
    fn counts_only_real_slides_ignoring_layouts_notes_and_rels() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("fileconv-probe-test-{}.pptx", std::process::id()));
        std::fs::write(&path, build_pptx(3)).unwrap();

        assert_eq!(count_pptx_slides(&path), Some(3));

        let _ = std::fs::remove_file(&path);
    }
}
