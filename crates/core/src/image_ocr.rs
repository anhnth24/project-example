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
    let local = PathBuf::from("tessdata_best");
    if local.join("vie.traineddata").exists() {
        return Some(local);
    }
    None
}

fn run_tesseract(path: &Path, langs: &str) -> io::Result<String> {
    let mut cmd = Command::new("tesseract");
    cmd.arg(path)
        .arg("stdout")
        .arg("-l")
        .arg(langs)
        .arg("--psm")
        .arg("3")
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

/// Kiểm tra tesseract có sẵn không.
pub fn tesseract_available() -> bool {
    Command::new("tesseract")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
