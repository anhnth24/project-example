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

#[derive(Debug, Deserialize, JsonSchema)]
struct TablesReq {
    /// Đường dẫn file Excel (xlsx/xls) hoặc CSV.
    path: String,
    /// (Excel) chỉ sheet theo tên. Bỏ trống = mọi sheet.
    #[serde(default)]
    sheet: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ChunksReq {
    /// Đường dẫn file cần convert + chia chunk.
    path: String,
    /// Số ký tự tối đa mỗi chunk (mặc định 2000).
    #[serde(default)]
    max_chars: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct LlmReq {
    /// Đường dẫn file cần tóm tắt.
    path: String,
    /// Giới hạn ký tự đưa vào LLM (kiểm soát chi phí). Mặc định 40000.
    #[serde(default)]
    max_chars: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ExtractReq {
    /// Đường dẫn file.
    path: String,
    /// Yêu cầu trích (ngôn ngữ tự nhiên), vd "lấy số hợp đồng, ngày ký, tổng tiền".
    instruction: String,
    #[serde(default)]
    max_chars: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TranslateReq {
    /// Đường dẫn file.
    path: String,
    /// Ngôn ngữ đích, vd "English", "tiếng Nhật".
    target: String,
    #[serde(default)]
    max_chars: Option<usize>,
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
    async fn detect_format(
        &self,
        Parameters(req): Parameters<DetectReq>,
    ) -> Result<String, String> {
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

    #[tool(
        description = "Trích bảng có cấu trúc thành JSON (tất định, không cần API key). Hỗ trợ Excel (xlsx/xls) và CSV. Excel: {\"<sheet>\": [[ô,...],...]}; CSV: [[ô,...],...]."
    )]
    async fn extract_tables_json(
        &self,
        Parameters(req): Parameters<TablesReq>,
    ) -> Result<String, String> {
        tokio::task::spawn_blocking(move || {
            fileconv_core::tables::tables_json(&PathBuf::from(&req.path), req.sheet.as_deref())
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())?
    }

    #[tool(
        description = "Convert file rồi CHIA thành chunks cho RAG/embedding: chia theo heading (giữ đường dẫn tiêu đề cha, vd 'Chương I > Điều 1'), section dài chia tiếp theo đoạn. Trả JSON [{index, heading, text, chars}]. Offline, không cần API key."
    )]
    async fn convert_chunks(
        &self,
        Parameters(req): Parameters<ChunksReq>,
    ) -> Result<String, String> {
        tokio::task::spawn_blocking(move || {
            let md = Converter::new()
                .convert_path(&PathBuf::from(&req.path))
                .map_err(|e| e.to_string())?
                .markdown;
            let chunks = fileconv_core::chunk::chunk_markdown(&md, req.max_chars.unwrap_or(2000));
            Ok(fileconv_core::chunk::chunks_json(&chunks))
        })
        .await
        .map_err(|e| e.to_string())?
    }

    #[tool(
        description = "Tóm tắt tài liệu (Markdown tiếng Việt). CẦN cấu hình LLM qua env FILECONV_LLM_* (provider + API key); chưa cấu hình sẽ báo lỗi."
    )]
    async fn summarize(&self, Parameters(req): Parameters<LlmReq>) -> Result<String, String> {
        tokio::task::spawn_blocking(move || {
            let cfg = llm_cfg()?;
            let md = convert_for_llm(&req.path, req.max_chars.unwrap_or(40_000))?;
            fileconv_core::llm::summarize(&cfg, &md).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())?
    }

    #[tool(
        description = "Trích dữ liệu có cấu trúc theo yêu cầu ngôn ngữ tự nhiên (vd 'lấy số hợp đồng, ngày, tổng tiền'), trả JSON. CẦN cấu hình LLM (FILECONV_LLM_*)."
    )]
    async fn extract_json(
        &self,
        Parameters(req): Parameters<ExtractReq>,
    ) -> Result<String, String> {
        tokio::task::spawn_blocking(move || {
            let cfg = llm_cfg()?;
            let md = convert_for_llm(&req.path, req.max_chars.unwrap_or(40_000))?;
            fileconv_core::llm::extract_json(&cfg, &md, &req.instruction).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())?
    }

    #[tool(
        description = "OCR tài liệu KHÓ bằng vision-LLM (ảnh đa cột, IN HOA mất dấu, chữ viết tay, con dấu) — chất lượng cao hơn Tesseract cho các ca này. Nhận file ảnh (png/jpg/webp). CẦN cấu hình LLM (FILECONV_LLM_*) với model vision; ảnh sẽ được gửi tới provider."
    )]
    async fn ocr_hard(&self, Parameters(req): Parameters<DetectReq>) -> Result<String, String> {
        tokio::task::spawn_blocking(move || {
            let cfg = llm_cfg()?;
            fileconv_core::llm::vision_ocr(&cfg, &PathBuf::from(&req.path))
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())?
    }

    #[tool(
        description = "Dịch nội dung tài liệu sang ngôn ngữ đích (target, vd 'English'). CẦN cấu hình LLM (FILECONV_LLM_*)."
    )]
    async fn translate(&self, Parameters(req): Parameters<TranslateReq>) -> Result<String, String> {
        tokio::task::spawn_blocking(move || {
            let cfg = llm_cfg()?;
            let md = convert_for_llm(&req.path, req.max_chars.unwrap_or(40_000))?;
            let instr = format!(
                "Dịch toàn bộ nội dung sau sang {}. Giữ định dạng Markdown, không thêm lời bình.",
                req.target
            );
            fileconv_core::llm::chat(
                &cfg,
                "Bạn là dịch giả chuyên nghiệp.",
                &format!("{instr}\n\n{md}"),
            )
            .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())?
    }
}

/// Lấy cấu hình LLM từ env, lỗi rõ nếu chưa cấu hình.
fn llm_cfg() -> Result<fileconv_core::llm::LlmConfig, String> {
    fileconv_core::llm::LlmConfig::from_env().ok_or_else(|| {
        "Chưa cấu hình LLM. Đặt env FILECONV_LLM_PROVIDER (openai|anthropic|gemini|openai-compatible) \
         và FILECONV_LLM_API_KEY (+ tuỳ chọn FILECONV_LLM_MODEL/BASE_URL)."
            .to_string()
    })
}

/// Convert file → Markdown (giới hạn ký tự để kiểm soát chi phí LLM).
fn convert_for_llm(path: &str, max_chars: usize) -> Result<String, String> {
    let opts = ConverterOptions {
        max_chars: Some(max_chars),
        ..ConverterOptions::default()
    };
    Converter::with_options(opts)
        .convert_path(&PathBuf::from(path))
        .map(|r| r.markdown)
        .map_err(|e| e.to_string())
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
