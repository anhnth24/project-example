//! Page render OCR and embedded-image OCR helpers (Tesseract via image_ocr).

use pdfium_render::prelude::*;

use crate::image_ocr::{self, OcrAttemptError, OcrRunConfig, OcrStage};

/// Trang có ít hơn ngưỡng này ký tự (không tính khoảng trắng) → coi là trang scan → OCR.
/// (Chỉ dùng ở đường fallback PDFium.)
pub(super) const PAGE_TEXT_MIN_CHARS: usize = 10;
/// Chỉ OCR ảnh nhúng đủ lớn (px²) — bỏ qua logo/icon nhỏ.
const MIN_IMG_AREA: i64 = 200 * 200;
/// DPI render trang khi OCR (cao hơn = OCR tốt hơn, chậm hơn).
const OCR_DPI: f32 = 300.0;

pub(super) enum PageOcr {
    Text(String),
    Blank,
}

/// Render + OCR một trang theo chỉ số 0-based.
pub(super) fn ocr_page_at(
    doc: &PdfDocument,
    page_0idx: u32,
    langs: &str,
    ocr_config: &OcrRunConfig,
) -> Result<PageOcr, OcrAttemptError> {
    let page = doc.pages().get(page_0idx as i32).map_err(|e| {
        OcrAttemptError::failed(
            OcrStage::Render,
            format!("trang {}: mở trang PDF thất bại: {e}", page_0idx + 1),
        )
    })?;
    ocr_full_page(&page, langs, ocr_config).map_err(|error| match error {
        OcrAttemptError::TesseractNotFound {
            stage,
            binary,
            message,
        } => OcrAttemptError::TesseractNotFound {
            stage,
            binary,
            message: format!("trang {}: {message}", page_0idx + 1),
        },
        OcrAttemptError::Failed {
            stage,
            message,
            io_kind,
        } => OcrAttemptError::Failed {
            stage,
            message: format!("trang {}: {message}", page_0idx + 1),
            io_kind,
        },
    })
}

/// OCR các ảnh nhúng đủ lớn trong một trang (cho trang trộn text + ảnh).
pub(super) fn ocr_page_images(
    doc: &PdfDocument,
    page: &PdfPage,
    langs: &str,
    page_no: usize,
    ocr_config: &OcrRunConfig,
    last_ocr_error: &mut Option<OcrAttemptError>,
) -> Option<String> {
    let mut out = String::new();
    for obj in page.objects().iter() {
        let Some(img_obj) = obj.as_image_object() else {
            continue;
        };
        let w = img_obj.width().unwrap_or(0) as i64;
        let h = img_obj.height().unwrap_or(0) as i64;
        if w * h < MIN_IMG_AREA {
            continue;
        }
        let Ok(img) = img_obj.get_processed_image(doc) else {
            continue;
        };
        match image_ocr::ocr_dynimage_detailed(&img, langs, ocr_config) {
            Ok(text) => {
                let text = text.trim();
                if text.chars().filter(|c| c.is_alphanumeric()).count() >= 4 {
                    out.push_str(&format!("<!-- Ảnh trong trang {page_no} (OCR) -->\n\n"));
                    out.push_str(text);
                    out.push_str("\n\n");
                }
            }
            Err(error) => {
                *last_ocr_error = Some(error);
            }
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Render cả trang ở OCR_DPI rồi OCR (qua image_ocr có tiền xử lý).
pub(super) fn rendered_page_is_blank(image: &image::DynamicImage) -> bool {
    let grayscale = image.to_luma8();
    let mut dark_pixels = 0usize;
    let mut max_row_dark = 0usize;
    for row in grayscale.rows() {
        let row_dark = row.filter(|pixel| pixel[0] < 200).count();
        dark_pixels += row_dark;
        max_row_dark = max_row_dark.max(row_dark);
    }
    dark_pixels.saturating_mul(1000) < grayscale.len()
        && max_row_dark <= (grayscale.width() as usize / 200).max(8)
}

pub(super) fn ocr_full_page(
    page: &PdfPage,
    langs: &str,
    ocr_config: &OcrRunConfig,
) -> Result<PageOcr, OcrAttemptError> {
    let w = (((page.width().value / 72.0) * OCR_DPI).round() as i32).clamp(100, 5000);
    let h = (((page.height().value / 72.0) * OCR_DPI).round() as i32).clamp(100, 7000);
    let bitmap = page
        .render(w, h, None)
        .map_err(|e| OcrAttemptError::failed(OcrStage::Render, format!("render: {e}")))?;
    let img = bitmap
        .as_image()
        .map_err(|e| OcrAttemptError::failed(OcrStage::Render, format!("as_image: {e}")))?;
    let text = image_ocr::ocr_dynimage_detailed(&img, langs, ocr_config)?;
    if !text.trim().is_empty() {
        Ok(PageOcr::Text(text))
    } else if rendered_page_is_blank(&img) {
        Ok(PageOcr::Blank)
    } else {
        Err(OcrAttemptError::failed(
            OcrStage::Tesseract,
            "Tesseract không trả nội dung cho trang có nét chữ",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::rendered_page_is_blank;

    #[test]
    fn distinguishes_blank_scan_noise_from_content() {
        let mut blank = image::GrayImage::from_pixel(1000, 1000, image::Luma([255]));
        for index in 0..500 {
            blank.put_pixel((index * 37) % 1000, (index * 53) % 1000, image::Luma([0]));
        }
        assert!(rendered_page_is_blank(&image::DynamicImage::ImageLuma8(
            blank
        )));

        let mut content = image::GrayImage::from_pixel(1000, 1000, image::Luma([255]));
        for y in 100..900 {
            for x in (100..900).step_by(20) {
                content.put_pixel(x, y, image::Luma([0]));
            }
        }
        assert!(!rendered_page_is_blank(&image::DynamicImage::ImageLuma8(
            content
        )));

        let mut sparse_line = image::GrayImage::from_pixel(1000, 1000, image::Luma([255]));
        for x in 450..550 {
            sparse_line.put_pixel(x, 500, image::Luma([0]));
        }
        assert!(!rendered_page_is_blank(&image::DynamicImage::ImageLuma8(
            sparse_line
        )));
    }
}
