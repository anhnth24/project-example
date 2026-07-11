//! OCR ảnh bằng Tesseract (gọi CLI `tesseract`), kèm **tiền xử lý ảnh** để tăng
//! độ chính xác (đặc biệt ảnh phân giải thấp).
//!
//! Tiền xử lý (thuần Rust, crate `image`): grayscale → chuẩn hoá KÍCH THƯỚC →
//! unsharp → normalize (kéo giãn histogram).
//!
//! Chuẩn hoá kích thước (quan trọng cho TỐC ĐỘ): trước đây phóng ×2 MỌI ảnh <2000px
//! khiến trang giấy tờ dày đặc bị đội gấp đôi thời gian OCR (vd 62s→88s) mà không tăng
//! độ chính xác. Nay:
//!   - Ảnh THỰC SỰ nhỏ/mờ (diện tích < ~0.5MP) → phóng ×2 (giúp OCR, mà vẫn rẻ vì ảnh nhỏ).
//!   - Ảnh quá lớn (cạnh dài > 2400px) → thu xuống (giữ tốc độ).
//!   - Còn lại (trang giấy tờ đủ nét) → GIỮ NGUYÊN.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use image::{imageops, DynamicImage, GrayImage};

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Diện tích (px²) dưới mức này ⇒ ảnh nhỏ/mờ ⇒ phóng ×2. ~0.5 megapixel.
const UPSCALE_IF_AREA_BELOW: u64 = 500_000;
/// Cạnh dài tối đa; lớn hơn ⇒ thu xuống để OCR không quá chậm.
const MAX_LONG_SIDE: u32 = 2400;

/// OCR một file ảnh. `langs` ví dụ "vie+eng".
pub fn ocr_image(path: &Path, langs: &str) -> io::Result<String> {
    match image::open(path) {
        Ok(img) => ocr_dynimage(&img, langs),
        // Không đọc/giải mã được bằng crate image → OCR thẳng file gốc.
        Err(_) => run_tesseract(path, langs),
    }
}

/// OCR một ảnh đã có trong bộ nhớ (vd trang PDF render ra) — có tiền xử lý.
pub fn ocr_dynimage(img: &DynamicImage, langs: &str) -> io::Result<String> {
    let pre = preprocess(img);
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("fileconv_ocr_{}_{seq}.png", std::process::id()));
    pre.save(&tmp)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
    let text = run_tesseract(&tmp, langs);
    let _ = std::fs::remove_file(&tmp);
    text
}

/// Tiền xử lý: grayscale → chuẩn hoá kích thước → unsharp → normalize.
fn preprocess(img: &DynamicImage) -> DynamicImage {
    let gray = img.to_luma8();
    let (w, h) = gray.dimensions();
    let area = w as u64 * h as u64;
    let long = w.max(h);

    let scaled = if area < UPSCALE_IF_AREA_BELOW {
        // Ảnh nhỏ/mờ → phóng ×2 (rẻ vì ảnh nhỏ), giúp OCR chính xác hơn.
        imageops::resize(&gray, w * 2, h * 2, imageops::FilterType::Lanczos3)
    } else if long > MAX_LONG_SIDE {
        // Ảnh quá lớn → thu xuống để giữ tốc độ.
        let f = MAX_LONG_SIDE as f32 / long as f32;
        imageops::resize(
            &gray,
            (w as f32 * f).round() as u32,
            (h as f32 * f).round() as u32,
            imageops::FilterType::Lanczos3,
        )
    } else {
        // Trang giấy tờ đủ nét → giữ nguyên (không đội thời gian OCR).
        gray
    };

    // Làm nét nhẹ rồi kéo giãn tương phản về [0,255].
    let mut sharp = imageops::unsharpen(&scaled, 1.0, 3);
    normalize(&mut sharp);
    DynamicImage::ImageLuma8(sharp)
}

/// Kéo giãn histogram grayscale về toàn dải [0,255].
fn normalize(buf: &mut GrayImage) {
    let (mut lo, mut hi) = (255u8, 0u8);
    for p in buf.pixels() {
        lo = lo.min(p[0]);
        hi = hi.max(p[0]);
    }
    if hi > lo {
        let range = (hi - lo) as f32;
        for p in buf.pixels_mut() {
            p[0] = (((p[0] - lo) as f32 / range) * 255.0).round() as u8;
        }
    }
}

/// Tìm thư mục tessdata chất lượng cao (tessdata_best) để tăng độ chính xác:
/// biến môi trường FILECONV_TESSDATA → ./tessdata_best (nếu có vie.traineddata).
/// Không có → dùng model hệ thống mặc định ("fast").
fn tessdata_dir() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("FILECONV_TESSDATA") {
        return Some(PathBuf::from(p));
    }

    let mut roots = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        roots.extend(cwd.ancestors().take(4).map(Path::to_path_buf));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            roots.extend(parent.ancestors().take(4).map(Path::to_path_buf));
        }
    }
    roots.extend(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .take(4)
            .map(Path::to_path_buf),
    );
    for root in roots {
        let candidate = root.join("tessdata_best");
        if candidate.join("vie.traineddata").exists() {
            return Some(candidate);
        }
    }
    None
}

fn run_tesseract(path: &Path, langs: &str) -> io::Result<String> {
    let automatic = run_tesseract_psm(path, langs, 3)?;
    if !should_retry_layout(&automatic) {
        return Ok(automatic);
    }
    let block = run_tesseract_psm(path, langs, 6)?;
    if ocr_text_score(&block) > ocr_text_score(&automatic) {
        Ok(block)
    } else {
        Ok(automatic)
    }
}

fn run_tesseract_psm(path: &Path, langs: &str, psm: u8) -> io::Result<String> {
    let mut cmd = Command::new("tesseract");
    cmd.arg(path)
        .arg("stdout")
        .arg("-l")
        .arg(langs)
        .arg("--psm")
        .arg(psm.to_string())
        .arg("--dpi")
        .arg("300");
    // Dùng model best nếu có (tăng độ chính xác tài liệu thật).
    if let Some(dir) = tessdata_dir() {
        cmd.env("TESSDATA_PREFIX", dir);
    }
    let output = cmd.output()?;

    if !output.status.success() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("tesseract lỗi: {}", String::from_utf8_lossy(&output.stderr)),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn should_retry_layout(text: &str) -> bool {
    let letters = text
        .chars()
        .filter(|character| character.is_alphabetic())
        .count();
    if letters < 12 || text.contains('\u{fffd}') {
        return true;
    }
    text.split_whitespace().any(|token| {
        let token_letters: Vec<char> = token
            .chars()
            .filter(|character| character.is_alphabetic())
            .collect();
        token_letters.len() >= 18
            && token_letters
                .iter()
                .filter(|character| character.is_uppercase())
                .count()
                * 10
                >= token_letters.len() * 9
    })
}

fn ocr_text_score(text: &str) -> i64 {
    let letters = text
        .chars()
        .filter(|character| character.is_alphabetic())
        .count() as i64;
    let words = text.split_whitespace().count() as i64;
    let replacements = text.matches('\u{fffd}').count() as i64;
    let glued_penalty: i64 = text
        .split_whitespace()
        .map(|token| {
            token
                .chars()
                .filter(|character| character.is_alphabetic())
                .count()
        })
        .filter(|length| *length > 18)
        .map(|length| (length - 18) as i64)
        .sum();
    letters * 3 + words * 4 - replacements * 30 - glued_penalty * 2
}

/// Kiểm tra tesseract có sẵn không.
pub fn tesseract_available() -> bool {
    Command::new("tesseract")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_sparse_replacement_and_glued_uppercase_output() {
        assert!(should_retry_layout("ít chữ"));
        assert!(should_retry_layout("Nội dung \u{fffd} bị lỗi encoding"));
        assert!(should_retry_layout(
            "BỘTƯPHÁPVÀCÁCCƠQUANLIÊNQUAN\nNội dung bình thường"
        ));
        assert!(!should_retry_layout(
            "BỘ TƯ PHÁP\nNội dung văn bản hành chính đầy đủ và rõ ràng."
        ));
    }

    #[test]
    fn quality_score_prefers_separated_clean_text() {
        let glued = "BỘTƯPHÁPVÀCÁCCƠQUANLIÊNQUAN";
        let clean = "BỘ TƯ PHÁP VÀ CÁC CƠ QUAN LIÊN QUAN";
        assert!(ocr_text_score(clean) > ocr_text_score(glued));
    }
}
