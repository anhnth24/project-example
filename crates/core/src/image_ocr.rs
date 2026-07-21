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

use std::cell::{Cell, RefCell};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use image::{imageops, DynamicImage, GrayImage, ImageReader, Limits};
use tempfile::NamedTempFile;

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
    static LAST_OCR_ERROR: RefCell<Option<String>> = const { RefCell::new(None) };
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

pub(crate) fn clear_last_ocr_error() {
    LAST_OCR_ERROR.with(|error| *error.borrow_mut() = None);
}

pub(crate) fn record_ocr_error(error: impl std::fmt::Display) {
    LAST_OCR_ERROR.with(|last| *last.borrow_mut() = Some(error.to_string()));
}

pub(crate) fn take_last_ocr_error() -> Option<String> {
    LAST_OCR_ERROR.with(|error| error.borrow_mut().take())
}

fn active_engine() -> OcrEngine {
    OCR_ENGINE.with(Cell::get)
}

/// Diện tích (px²) dưới mức này ⇒ ảnh nhỏ/mờ ⇒ phóng ×2. ~0.5 megapixel.
const UPSCALE_IF_AREA_BELOW: u64 = 500_000;
/// Cạnh dài tối đa; lớn hơn ⇒ thu xuống để OCR không quá chậm.
const MAX_LONG_SIDE: u32 = 2400;
/// Cạnh tối đa khi decode (strict, qua `image::Limits`). Cho phép trang lớn
/// (A3@~600 DPI ≈ 5k×7k, A1@300 DPI ≈ 10k) nhưng chặn decompression bomb.
/// Giữ `Limits::default().max_alloc` (512 MiB) của crate `image`.
const MAX_DECODE_SIDE: u32 = 12_000;

/// Limits decode OCR: giữ max_alloc mặc định của `image`, thêm trần cạnh.
fn ocr_image_limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DECODE_SIDE);
    limits.max_image_height = Some(MAX_DECODE_SIDE);
    limits
}

fn image_error_to_io(error: image::ImageError) -> io::Error {
    match error {
        image::ImageError::IoError(inner) => inner,
        image::ImageError::Limits(_) => {
            io::Error::new(io::ErrorKind::InvalidData, error.to_string())
        }
        other => io::Error::new(io::ErrorKind::Other, other.to_string()),
    }
}

fn is_image_limit_error(error: &image::ImageError) -> bool {
    matches!(error, image::ImageError::Limits(_))
}

fn ensure_ocr_image_bounds(width: u32, height: u32) -> io::Result<()> {
    ocr_image_limits()
        .check_dimensions(width, height)
        .map_err(image_error_to_io)
}

/// Mở ảnh với giới hạn dimension/alloc **trước** khi decode đầy đủ buffer.
fn load_image_for_ocr(path: &Path) -> Result<DynamicImage, image::ImageError> {
    let mut reader = ImageReader::open(path)?;
    reader.limits(ocr_image_limits());
    reader.decode()
}

/// Ghi PNG OCR vào temp exclusive (`O_EXCL`/random name) — tránh path đoán được.
fn write_ocr_temp_png(img: &DynamicImage) -> io::Result<NamedTempFile> {
    let mut tmp = tempfile::Builder::new()
        .prefix("fileconv_ocr_")
        .suffix(".png")
        .tempfile()?;
    img.write_to(&mut tmp, image::ImageFormat::Png)
        .map_err(image_error_to_io)?;
    tmp.flush()?;
    Ok(tmp)
}

fn write_ocr_temp_gray_png(img: &GrayImage) -> io::Result<NamedTempFile> {
    write_ocr_temp_png(&DynamicImage::ImageLuma8(img.clone()))
}

/// OCR một file ảnh. `langs` ví dụ "vie+eng".
pub fn ocr_image(path: &Path, langs: &str) -> io::Result<String> {
    match load_image_for_ocr(path) {
        Ok(img) => ocr_dynimage(&img, langs),
        // Vượt giới hạn kích thước/alloc → fail rõ, KHÔNG đẩy bomb sang tesseract.
        Err(error) if is_image_limit_error(&error) => {
            let error = image_error_to_io(error);
            record_ocr_error(&error);
            Err(error)
        }
        // Không đọc/giải mã được bằng crate image → OCR thẳng file gốc.
        Err(_) => run_tesseract(path, langs),
    }
}

/// OCR một ảnh đã có trong bộ nhớ (vd trang PDF render ra) — có tiền xử lý.
pub fn ocr_dynimage(img: &DynamicImage, langs: &str) -> io::Result<String> {
    ensure_ocr_image_bounds(img.width(), img.height())?;
    let pre = preprocess(img);
    let tmp = write_ocr_temp_png(&pre)?;
    let tmp_path = tmp.path();
    let tesseract = || run_tesseract_with_columns(&pre.to_luma8(), tmp_path, langs);
    let text = match active_engine() {
        OcrEngine::Tesseract => tesseract(),
        OcrEngine::Paddle => run_paddle(tmp_path, langs).or_else(|_| tesseract()),
        OcrEngine::Auto => {
            let baseline = tesseract()?;
            if should_retry_layout(&baseline) {
                match run_paddle(tmp_path, langs) {
                    Ok(paddle) if ocr_text_score(&paddle) > ocr_text_score(&baseline) => Ok(paddle),
                    _ => Ok(baseline),
                }
            } else {
                Ok(baseline)
            }
        }
    };
    // `tmp` drop → xoá exclusive temp; không dùng path đoán được theo pid/seq.
    drop(tmp);
    if let Err(error) = &text {
        record_ocr_error(error);
    }
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
    for (left, right) in ranges {
        let cropped = imageops::crop_imm(image, left, 0, right - left, image.height()).to_image();
        let tmp = write_ocr_temp_gray_png(&cropped)?;
        let path = tmp.path();
        let automatic = run_tesseract_psm(path, langs, 4);
        let text = match automatic {
            Ok(value) if should_retry_layout(&value) => {
                let block = run_tesseract_psm(path, langs, 6).unwrap_or_default();
                if ocr_text_score(&block) > ocr_text_score(&value) {
                    block
                } else {
                    value
                }
            }
            Ok(value) => value,
            Err(error) => return Err(error),
        };
        drop(tmp);
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
    run_tesseract_psm_with_binary(&tesseract_binary(), path, langs, psm)
}

/// Xây `Command` tesseract (argv rời, không shell). `binary` inject được cho test.
fn build_tesseract_psm_command(binary: &Path, path: &Path, langs: &str, psm: u8) -> Command {
    let mut cmd = crate::proc::background_command(binary);
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
    cmd
}

/// Marker set only when `Command::output` for Tesseract returns `NotFound`.
#[derive(Debug)]
struct TesseractNotFoundError {
    message: String,
}

impl std::fmt::Display for TesseractNotFoundError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for TesseractNotFoundError {}

/// True only when the error was tagged at the Tesseract spawn-`NotFound` stage.
pub fn error_is_tesseract_not_found(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|inner| inner.is::<TesseractNotFoundError>())
}

fn run_tesseract_psm_with_binary(
    binary: &Path,
    path: &Path,
    langs: &str,
    psm: u8,
) -> io::Result<String> {
    let output = match build_tesseract_psm_command(binary, path, langs, psm).output() {
        Ok(output) => output,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                TesseractNotFoundError {
                    message: format!(
                        "không tìm thấy binary Tesseract ({}): {error}",
                        binary.display()
                    ),
                },
            ));
        }
        Err(error) => return Err(error),
    };

    if !output.status.success() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("tesseract lỗi: {}", String::from_utf8_lossy(&output.stderr)),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Resolve binary: `FILECONV_TESSERACT` override (nếu non-empty) → `tesseract`.
fn resolve_tesseract_binary(override_bin: Option<&std::ffi::OsStr>) -> PathBuf {
    override_bin
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("tesseract"))
}

fn tesseract_binary() -> PathBuf {
    resolve_tesseract_binary(std::env::var_os("FILECONV_TESSERACT").as_deref())
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

/// Kiểm tra Tesseract hệ thống hoặc runtime desktop đi kèm có sẵn không.
pub fn tesseract_available() -> bool {
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

    #[test]
    fn decode_limits_keep_image_default_alloc_and_allow_scanned_pages() {
        let limits = ocr_image_limits();
        assert_eq!(limits.max_alloc, Limits::default().max_alloc);
        assert_eq!(limits.max_image_width, Some(MAX_DECODE_SIDE));
        assert_eq!(limits.max_image_height, Some(MAX_DECODE_SIDE));
        // A4 @ 300 DPI and A3 @ ~600 DPI must remain acceptable.
        assert!(ensure_ocr_image_bounds(2480, 3508).is_ok());
        assert!(ensure_ocr_image_bounds(4961, 7016).is_ok());
        assert!(ensure_ocr_image_bounds(MAX_DECODE_SIDE, MAX_DECODE_SIDE).is_ok());
        assert!(ensure_ocr_image_bounds(MAX_DECODE_SIDE + 1, 100).is_err());
        assert!(ensure_ocr_image_bounds(100, MAX_DECODE_SIDE + 1).is_err());
    }

    #[test]
    fn load_image_rejects_oversized_png_header_before_full_decode() {
        // IHDR-only PNG claiming 20000×20000 — dimension check fails before pixel decode.
        let bytes = hex_literal_png_ihdr(20_000, 20_000);
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("bomb.png");
        std::fs::write(&path, bytes).expect("write bomb png");
        let err = load_image_for_ocr(&path).expect_err("oversized header must fail");
        assert!(is_image_limit_error(&err), "got {err:?}");
    }

    #[test]
    fn ocr_image_does_not_fallback_tesseract_on_dimension_limit() {
        // Limit path must fail before any tesseract spawn (no stub required).
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("bomb.png");
        std::fs::write(&path, hex_literal_png_ihdr(20_000, 20_000)).expect("write");
        let err = ocr_image(&path, "eng").expect_err("limit must surface");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(
            take_last_ocr_error().is_some_and(|message| message.contains("limit")),
            "limit failure should be recorded"
        );
    }

    #[test]
    fn ocr_temp_png_uses_exclusive_random_path() {
        let img = DynamicImage::ImageLuma8(GrayImage::from_pixel(16, 16, image::Luma([128])));
        let first = write_ocr_temp_png(&img).expect("temp 1");
        let second = write_ocr_temp_png(&img).expect("temp 2");
        assert_ne!(first.path(), second.path());
        let name = first
            .path()
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        assert!(name.starts_with("fileconv_ocr_"));
        assert!(name.ends_with(".png"));
        // Old predictable pattern was `fileconv_ocr_{pid}_{seq}.png`.
        assert!(
            !name
                .trim_start_matches("fileconv_ocr_")
                .starts_with(&format!("{}_", std::process::id())),
            "temp name should not be pid-prefixed: {name}"
        );
        assert!(first.path().exists());
        let path = first.path().to_path_buf();
        drop(first);
        assert!(!path.exists(), "NamedTempFile must unlink on drop");
    }

    #[test]
    fn resolve_tesseract_binary_honors_override_and_default() {
        assert_eq!(resolve_tesseract_binary(None).as_os_str(), "tesseract");
        assert_eq!(
            resolve_tesseract_binary(Some(std::ffi::OsStr::new(""))).as_os_str(),
            "tesseract"
        );
        assert_eq!(
            resolve_tesseract_binary(Some(std::ffi::OsStr::new("/opt/custom-tess"))),
            PathBuf::from("/opt/custom-tess")
        );
    }

    /// Injected binary must receive the OCR tempfile as argv[1] and be able to read it.
    #[cfg(unix)]
    #[test]
    fn injected_tesseract_binary_opens_ocr_tempfile() {
        let dir = tempfile::tempdir().expect("tempdir");
        let stub = install_readable_tesseract_stub(dir.path());
        let img = DynamicImage::ImageLuma8(GrayImage::from_pixel(32, 32, image::Luma([40])));
        let tmp = write_ocr_temp_png(&img).expect("ocr temp");
        let text =
            run_tesseract_psm_with_binary(&stub, tmp.path(), "eng", 3).expect("injected stub OCR");
        assert!(
            text.contains("FILECONV_TESSERACT_STUB_OK"),
            "unexpected stub output: {text:?}"
        );
        assert!(
            text.contains("READ_OK"),
            "stub must confirm argv[1] was readable: {text:?}"
        );
    }

    /// Production `FILECONV_TESSERACT` override: set only on a child process via
    /// `Command::env` — never mutate the in-process environment (parallel-safe).
    #[cfg(unix)]
    #[test]
    fn ocr_image_honors_fileconv_tesseract_env_in_child() {
        const CHILD_FLAG: &str = "FILECONV_OCR_TEST_CHILD";
        const CHILD_IMAGE: &str = "FILECONV_OCR_TEST_IMAGE";

        if std::env::var_os(CHILD_FLAG).is_some() {
            let image = std::env::var(CHILD_IMAGE).expect("child image path");
            let text = ocr_image(Path::new(&image), "eng").expect("child OCR via env override");
            assert!(
                text.contains("FILECONV_TESSERACT_STUB_OK") && text.contains("READ_OK"),
                "child OCR output: {text:?}"
            );
            return;
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let stub = install_readable_tesseract_stub(dir.path());
        let image_path = dir.path().join("sample.png");
        DynamicImage::ImageLuma8(GrayImage::from_pixel(64, 64, image::Luma([0])))
            .save(&image_path)
            .expect("sample png");

        let exe = std::env::current_exe().expect("test executable");
        let output = Command::new(&exe)
            .args([
                "--exact",
                "image_ocr::tests::ocr_image_honors_fileconv_tesseract_env_in_child",
            ])
            .env(CHILD_FLAG, "1")
            .env(CHILD_IMAGE, &image_path)
            .env("FILECONV_TESSERACT", &stub)
            .env("RUST_TEST_THREADS", "1")
            .output()
            .expect("spawn child test process");
        assert!(
            output.status.success(),
            "child failed status={:?}\nstdout={}\nstderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Minimal PNG with only an IHDR chunk (precomputed CRC for given size).
    fn hex_literal_png_ihdr(width: u32, height: u32) -> Vec<u8> {
        // Signature + IHDR length/type/data/CRC for 20000×20000 grayscale.
        match (width, height) {
            (20_000, 20_000) => vec![
                0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
                0x44, 0x52, 0x00, 0x00, 0x4e, 0x20, 0x00, 0x00, 0x4e, 0x20, 0x08, 0x00, 0x00, 0x00,
                0x00, 0xc6, 0x1b, 0x19, 0xe5,
            ],
            _ => panic!("add CRC fixture for {width}x{height}"),
        }
    }

    /// Unix stub: open/read argv[1] (image path), then print markers to stdout.
    #[cfg(unix)]
    fn install_readable_tesseract_stub(dir: &Path) -> PathBuf {
        let stub = dir.join("fake-tesseract");
        // Portable POSIX: require readable argv[1], read 8 bytes (PNG sig), then OK.
        std::fs::write(
            &stub,
            "#!/bin/sh\n\
             set -eu\n\
             image=${1:-}\n\
             if [ -z \"$image\" ] || [ ! -r \"$image\" ]; then\n\
               echo \"stub: image not readable: ${image:-<missing>}\" >&2\n\
               exit 2\n\
             fi\n\
             head -c 8 \"$image\" >/dev/null\n\
             printf '%s\\n' 'READ_OK FILECONV_TESSERACT_STUB_OK'\n",
        )
        .expect("write stub");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        stub
    }
}
