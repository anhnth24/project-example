//! OCR ảnh bằng Tesseract (gọi qua CLI `tesseract`).
//!
//! Cách tiếp cận: gọi binary `tesseract` (offline, hỗ trợ `vie`) thay vì link
//! native leptess/tesseract-rs — đơn giản, không phụ thuộc build, dễ kiểm thử.
//! Khi đóng gói desktop app sẽ chuyển sang bundling tesseract-rs/leptess.

use std::io;
use std::path::Path;
use std::process::Command;

/// OCR một ảnh, trả về text. `langs` ví dụ "vie+eng".
pub fn ocr_image(path: &Path, langs: &str) -> io::Result<String> {
    let output = Command::new("tesseract")
        .arg(path)
        .arg("stdout")
        .arg("-l")
        .arg(langs)
        .arg("--psm")
        .arg("3") // tự động phân trang
        .output()?;

    if !output.status.success() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!(
                "tesseract lỗi: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
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
