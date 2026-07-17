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

use std::cell::Cell;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use image::{imageops, DynamicImage, GrayImage};

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);
static TESSERACT_AVAILABLE: OnceLock<bool> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OcrEngine {
    Tesseract,
    Paddle,
    Auto,
}

impl OcrEngine {
    pub fn from_name(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "paddle" | "paddleocr" => Self::Paddle,
            "auto" => Self::Auto,
            _ => Self::Tesseract,
        }
    }
}

thread_local! {
    static OCR_ENGINE: Cell<OcrEngine> = const { Cell::new(OcrEngine::Tesseract) };
}

pub fn with_ocr_engine<T>(engine: OcrEngine, operation: impl FnOnce() -> T) -> T {
    OCR_ENGINE.with(|active| {
        let previous = active.replace(engine);
        struct Reset<'a>(&'a Cell<OcrEngine>, OcrEngine);
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                self.0.set(self.1);
            }
        }
        let _reset = Reset(active, previous);
        operation()
    })
}

fn active_engine() -> OcrEngine {
    OCR_ENGINE.with(Cell::get)
}

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
    let tesseract = || run_tesseract_with_columns(&pre.to_luma8(), &tmp, langs);
    let text = match active_engine() {
        OcrEngine::Tesseract => tesseract(),
        OcrEngine::Paddle => run_paddle(&tmp, langs).or_else(|_| tesseract()),
        OcrEngine::Auto => {
            let baseline = tesseract()?;
            if should_retry_layout(&baseline) {
                match run_paddle(&tmp, langs) {
                    Ok(paddle) if ocr_text_score(&paddle) > ocr_text_score(&baseline) => Ok(paddle),
                    _ => Ok(baseline),
                }
            } else {
                Ok(baseline)
            }
        }
    };
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

fn detect_column_ranges(image: &GrayImage) -> Vec<(u32, u32)> {
    let (width, height) = image.dimensions();
    if width < 800 || height < 500 {
        return vec![(0, width)];
    }
    let y_start = height / 30;
    let y_end = height.saturating_sub(y_start);
    let max_gutter_ink = ((y_end - y_start) / 220).max(2);
    let min_gutter_width = (width / 70).max(12);
    let mut projection = vec![0u32; width as usize];
    for x in 0..width {
        projection[x as usize] = (y_start..y_end)
            .filter(|y| image.get_pixel(x, *y)[0] < 205)
            .count() as u32;
    }

    let mut gutters = Vec::new();
    let mut start = None;
    for x in width / 10..width * 9 / 10 {
        if projection[x as usize] <= max_gutter_ink {
            start.get_or_insert(x);
        } else if let Some(left) = start.take() {
            if x - left >= min_gutter_width {
                gutters.push((left, x));
            }
        }
    }
    if let Some(left) = start {
        let right = width * 9 / 10;
        if right.saturating_sub(left) >= min_gutter_width {
            gutters.push((left, right));
        }
    }

    gutters.sort_by_key(|(left, right)| std::cmp::Reverse(right - left));
    gutters.truncate(2);
    gutters.sort_by_key(|(left, _)| *left);
    let cuts: Vec<u32> = gutters
        .iter()
        .map(|(left, right)| left + (right - left) / 2)
        .collect();
    if cuts.is_empty() {
        return vec![(0, width)];
    }
    let mut ranges = Vec::new();
    let mut left = 0;
    for cut in cuts {
        if cut.saturating_sub(left) < width / 6 {
            continue;
        }
        ranges.push((left, cut));
        left = cut;
    }
    if width.saturating_sub(left) >= width / 6 {
        ranges.push((left, width));
    }
    if !(2..=3).contains(&ranges.len()) {
        return vec![(0, width)];
    }

    let contentful = ranges.iter().all(|(left, right)| {
        projection[*left as usize..*right as usize]
            .iter()
            .sum::<u32>()
            > height / 2
    });
    if contentful {
        ranges
    } else {
        vec![(0, width)]
    }
}

fn run_tesseract_with_columns(
    image: &GrayImage,
    whole_path: &Path,
    langs: &str,
) -> io::Result<String> {
    let whole = run_tesseract(whole_path, langs)?;
    let ranges = detect_column_ranges(image);
    if ranges.len() <= 1 {
        return Ok(whole);
    }
    let mut columns = Vec::new();
    for (column, (left, right)) in ranges.into_iter().enumerate() {
        let cropped = imageops::crop_imm(image, left, 0, right - left, image.height()).to_image();
        let sequence = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "fileconv_ocr_{}_{}_col{}.png",
            std::process::id(),
            sequence,
            column + 1
        ));
        cropped
            .save(&path)
            .map_err(|error| io::Error::new(io::ErrorKind::Other, error.to_string()))?;
        let automatic = run_tesseract_psm(&path, langs, 4);
        let text = match automatic {
            Ok(value) if should_retry_layout(&value) => {
                let block = run_tesseract_psm(&path, langs, 6).unwrap_or_default();
                if ocr_text_score(&block) > ocr_text_score(&value) {
                    block
                } else {
                    value
                }
            }
            Ok(value) => value,
            Err(error) => {
                let _ = std::fs::remove_file(path);
                return Err(error);
            }
        };
        let _ = std::fs::remove_file(path);
        if !text.trim().is_empty() {
            columns.push(text.trim().to_string());
        }
    }
    let split = columns.join("\n\n");
    if columns.len() >= 2 && ocr_text_score(&split) * 100 >= ocr_text_score(&whole) * 85 {
        Ok(split)
    } else {
        Ok(whole)
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

fn paddle_script() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("FILECONV_PADDLE_SCRIPT") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let mut roots = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        roots.extend(cwd.ancestors().take(5).map(Path::to_path_buf));
    }
    if let Ok(executable) = std::env::current_exe() {
        if let Some(parent) = executable.parent() {
            roots.extend(parent.ancestors().take(5).map(Path::to_path_buf));
        }
    }
    roots.extend(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .take(5)
            .map(Path::to_path_buf),
    );
    roots
        .into_iter()
        .map(|root| root.join("bench/paddle_ocr_cli.py"))
        .find(|path| path.is_file())
}

#[derive(serde::Deserialize)]
struct PaddleOutput {
    text: String,
}

fn run_paddle(path: &Path, langs: &str) -> io::Result<String> {
    let script = paddle_script().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "không tìm thấy PaddleOCR wrapper; đặt FILECONV_PADDLE_SCRIPT",
        )
    })?;
    let language = if langs.split('+').any(|lang| lang == "vie") {
        "vi"
    } else {
        "en"
    };
    let output = crate::proc::background_command("python3")
        .arg(script)
        .arg("--image")
        .arg(path)
        .arg("--lang")
        .arg(language)
        .env("PADDLE_PDX_DISABLE_MODEL_SOURCE_CHECK", "True")
        .output()?;
    if !output.status.success() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "PaddleOCR lỗi; kiểm tra paddleocr/paddlepaddle và model local",
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: PaddleOutput = stdout
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str(line).ok())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "PaddleOCR không trả JSON hợp lệ",
            )
        })?;
    if parsed.text.trim().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "PaddleOCR không trả text",
        ));
    }
    Ok(parsed.text)
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
    let binary = tesseract_binary();
    let mut cmd = crate::proc::background_command(&binary);
    apply_ocr_runtime_env(&mut cmd);
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

fn tesseract_binary() -> PathBuf {
    std::env::var_os("FILECONV_TESSERACT")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("tesseract"))
}

fn apply_ocr_runtime_env(command: &mut Command) {
    let Some(runtime_lib) = std::env::var_os("FILECONV_OCR_LIB_DIR") else {
        return;
    };
    #[cfg(target_os = "linux")]
    {
        let mut paths = vec![PathBuf::from(runtime_lib)];
        if let Some(existing) = std::env::var_os("LD_LIBRARY_PATH") {
            paths.extend(std::env::split_paths(&existing));
        }
        if let Ok(value) = std::env::join_paths(paths) {
            command.env("LD_LIBRARY_PATH", value);
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = runtime_lib;
}

pub(crate) fn tesseract_available() -> bool {
    *TESSERACT_AVAILABLE.get_or_init(|| {
        let binary = tesseract_binary();
        let mut command = Command::new(binary);
        apply_ocr_runtime_env(&mut command);
        command
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
    })
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
    crate::proc::background_command("tesseract")
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

    #[test]
    fn detects_two_content_columns_but_not_one_wide_block() {
        let mut two_columns = GrayImage::from_pixel(1200, 800, image::Luma([255]));
        for &(left, right) in &[(100, 470), (730, 1100)] {
            for y in 80..720 {
                if (y / 45) % 2 == 0 {
                    for x in left..right {
                        two_columns.put_pixel(x, y, image::Luma([0]));
                    }
                }
            }
        }
        let ranges = detect_column_ranges(&two_columns);
        assert_eq!(ranges.len(), 2);
        assert!(ranges[0].1 < ranges[1].1);

        let mut single = GrayImage::from_pixel(1200, 800, image::Luma([255]));
        for y in 80..720 {
            if (y / 45) % 2 == 0 {
                for x in 180..1020 {
                    single.put_pixel(x, y, image::Luma([0]));
                }
            }
        }
        assert_eq!(detect_column_ranges(&single).len(), 1);
    }

    #[test]
    fn parses_ocr_engine_names_with_safe_default() {
        assert_eq!(OcrEngine::from_name("paddle"), OcrEngine::Paddle);
        assert_eq!(OcrEngine::from_name("auto"), OcrEngine::Auto);
        assert_eq!(OcrEngine::from_name("unknown"), OcrEngine::Tesseract);
    }
}
