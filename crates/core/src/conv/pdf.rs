//! PDF → Markdown (text). Dùng `pdf-extract`.
//!
//! `pdf-extract` có thể **panic** (index out of bounds) trên một số PDF phức tạp.
//! Để backend không sập vì một file lỗi, ta bắt panic bằng `catch_unwind` và trả
//! về `ConvertError` bình thường.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

use super::fail;
use crate::ConvertError;

pub fn to_markdown(path: &Path) -> Result<String, ConvertError> {
    let bytes = std::fs::read(path).map_err(fail)?;
    let result = catch_unwind(AssertUnwindSafe(|| {
        pdf_extract::extract_text_from_mem(&bytes)
    }));
    match result {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(e)) => Err(fail(e)),
        Err(_) => Err(ConvertError::Failed(
            "pdf-extract panic (PDF phức tạp/không chuẩn)".to_string(),
        )),
    }
}
