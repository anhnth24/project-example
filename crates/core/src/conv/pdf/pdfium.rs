//! PDFium native binding: process-wide init mutex, call mutex, and thread-local cache.
//!
//! libpdfium is not thread-safe across concurrent FPDF calls. All PDFium use
//! must hold [`pdfium_call_guard`] for the duration of the call region.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use pdfium_render::prelude::*;

thread_local! {
    // PDFium chỉ init MỘT lần/tiến trình → cache một instance mỗi thread.
    static PDFIUM: Option<Pdfium> = load_pdfium();
}

static PDFIUM_INIT: std::sync::Mutex<()> = std::sync::Mutex::new(());

// libpdfium KHÔNG thread-safe: hai conversion song song (watch worker + lệnh
// convert desktop qua spawn_blocking) gọi FPDF đan xen vào state C toàn cục
// → UB/crash. Feature `thread_safe` của pdfium-render chỉ chia sẻ binding qua
// OnceCell, không khóa từng lời gọi, nên mọi vùng đụng PDFium phải giữ khóa
// này suốt vùng đó. Khóa ôm cả đoạn OCR trang scan cho đơn giản — hai PDF scan
// convert song song sẽ xếp hàng ở đoạn render+OCR; nếu throughput thành vấn đề
// thì tách Tesseract ra ngoài khóa.
static PDFIUM_CALL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Giữ khóa serialize PDFium trong suốt lifetime của guard trả về.
pub(super) fn pdfium_call_guard() -> std::sync::MutexGuard<'static, ()> {
    PDFIUM_CALL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(super) fn pdfium_available() -> bool {
    with_pdfium(|pdfium| pdfium.is_some())
}

/// Run `f` with the thread-local PDFium instance (if loaded).
pub(super) fn with_pdfium<R>(f: impl FnOnce(Option<&Pdfium>) -> R) -> R {
    PDFIUM.with(|opt| f(opt.as_ref()))
}

/// Bind libpdfium (nếu có).
pub(super) fn load_pdfium() -> Option<Pdfium> {
    let _init_guard = PDFIUM_INIT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(p) = std::env::var("FILECONV_PDFIUM_LIB") {
        let path = PathBuf::from(p);
        candidates.push(path.clone());
        if path.is_dir() {
            candidates.push(Pdfium::pdfium_platform_library_name_at_path(&path));
        }
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
        candidates.push(Pdfium::pdfium_platform_library_name_at_path(
            &root.join("pdfium/lib"),
        ));
        candidates.push(Pdfium::pdfium_platform_library_name_at_path(
            &root.join("pdfium"),
        ));
    }
    let mut seen = HashSet::new();
    candidates.retain(|path| seen.insert(path.clone()));

    for c in candidates {
        match Pdfium::bind_to_library(c) {
            Ok(bindings) => return Some(Pdfium::new(bindings)),
            Err(PdfiumError::PdfiumLibraryBindingsAlreadyInitialized) => {
                return Some(Pdfium::default());
            }
            Err(_) => {}
        }
    }
    match Pdfium::bind_to_system_library() {
        Ok(bindings) => Some(Pdfium::new(bindings)),
        Err(PdfiumError::PdfiumLibraryBindingsAlreadyInitialized) => Some(Pdfium::default()),
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::load_pdfium;

    #[test]
    fn reuses_initialized_pdfium_bindings_across_threads() {
        if load_pdfium().is_none() {
            return; // PDFium is an optional runtime dependency.
        }
        let handles: Vec<_> = (0..4)
            .map(|_| std::thread::spawn(|| load_pdfium().is_some()))
            .collect();
        assert!(handles
            .into_iter()
            .all(|handle| handle.join().unwrap_or(false)));
    }
}
