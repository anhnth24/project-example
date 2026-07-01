//! fileconv-mcp — MCP server (stdio) cho fileconv-core.
//!
//! Cho AI/agent chuyển file → Markdown NGOÀI context (đỡ tốn token). Tất định, offline,
//! không cần API key. Tool:
//!   - `detect_format(path)`       — xem loại/kích thước/số trang/sheet (không convert).
//!   - `convert_to_markdown(path)` — convert; hỗ trợ chọn trang (pages), sheet, max_chars.
//!
//! Đường dẫn tài nguyên qua env: FILECONV_PDFIUM_LIB, FILECONV_TESSDATA, FILECONV_WHISPER_MODEL.

use std::path::PathBuf;

use fileconv_core::{Converter, ConverterOptions};
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{Implementation, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
    transport::stdio,
    ServerHandler, ServiceExt,
};
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
struct DetectReq {
    /// Đường dẫn tuyệt đối tới file cần xem.
    path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ConvertReq {
    /// Đường dẫn tuyệt đối tới file (pdf/docx/pptx/xlsx/csv/html/ảnh/audio).
    path: String,
    /// (PDF) chỉ trích các trang này, 1-indexed. Bỏ trống = mọi trang.
    #[serde(default)]
    pages: Option<Vec<u32>>,
    /// (Excel) chỉ trích sheet theo tên. Bỏ trống = mọi sheet.
    #[serde(default)]
    sheet: Option<String>,
    /// Giới hạn số ký tự Markdown trả về (cắt kèm chú thích). Bỏ trống = không cắt.
    #[serde(default)]
    max_chars: Option<usize>,
    /// Ngôn ngữ OCR ảnh, vd "vie+eng". Mặc định "vie+eng".
    #[serde(default)]
    ocr_langs: Option<String>,
}

#[derive(Clone)]
struct Fileconv {
    tool_router: ToolRouter<Fileconv>,
}

#[tool_router(router = tool_router)]
impl Fileconv {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Xem metadata một file (loại, kích thước, số trang PDF / số slide PPTX / danh sách sheet Excel) mà KHÔNG convert — rẻ, giúp quyết định trích phần nào."
    )]
    async fn detect_format(&self, Parameters(req): Parameters<DetectReq>) -> Result<String, String> {
        let info = fileconv_core::probe(&PathBuf::from(&req.path));
        Ok(serde_json::json!({
            "format": info.format.as_str(),
            "bytes": info.bytes,
            "pages": info.pages,
            "sheets": info.sheets,
        })
        .to_string())
    }

    #[tool(
        description = "Chuyển file sang Markdown (pdf/docx/pptx/xlsx/csv/html + ảnh OCR + audio). Chạy offline. Dùng `pages` (PDF, 1-indexed), `sheet` (Excel) hoặc `max_chars` để chỉ lấy phần cần — tiết kiệm token."
    )]
    async fn convert_to_markdown(
        &self,
        Parameters(req): Parameters<ConvertReq>,
    ) -> Result<String, String> {
        tokio::task::spawn_blocking(move || {
            let mut opts = ConverterOptions {
                pdf_pages: req.pages,
                xlsx_sheet: req.sheet,
                max_chars: req.max_chars,
                ..ConverterOptions::default()
            };
            if let Some(l) = req.ocr_langs {
                opts.ocr_langs = l;
            }
            if let Ok(m) = std::env::var("FILECONV_WHISPER_MODEL") {
                opts.whisper_model = Some(PathBuf::from(m));
            }
            Converter::with_options(opts)
                .convert_path(&PathBuf::from(&req.path))
                .map(|r| r.markdown)
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())?
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Fileconv {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "fileconv-mcp".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                ..Default::default()
            },
            instructions: Some(
                "Chuyển file sang Markdown offline (pdf/docx/pptx/xlsx/csv/html/ảnh OCR/audio). \
                 Gọi detect_format để xem trước (số trang/sheet), rồi convert_to_markdown; \
                 truyền pages/sheet/max_chars để chỉ lấy phần cần, tiết kiệm token."
                    .into(),
            ),
            ..Default::default()
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let service = Fileconv::new().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
