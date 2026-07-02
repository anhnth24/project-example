# Research: Tool tương tự & giải pháp tốt nhất (07/2026)

> Deep-research qua 5 nhánh (open-source converters, ML parsers, cloud APIs, landscape khác,
> góc tiếng Việt). Nguồn chi tiết + độ tin cậy từng claim nằm trong transcript research;
> dưới đây là tổng hợp hành động được.

## 1. Bảng so sánh nhanh

| Tool | Loại | License | Điểm mạnh | Điểm yếu (vs mình) |
|---|---|---|---|---|
| **MarkItDown** (Microsoft, ~150k⭐) | Python lib+CLI+MCP | MIT | 15+ formats, plugin, MCP chuẩn | PDF **0.000 điểm heading** (benchmark yage.ai), lỗi Unicode với text non-ASCII |
| **Pandoc** | CLI Haskell | GPL | 60+ format, AST lossless | **Không PDF input, không OCR/audio** |
| **Docling** (IBM, ~62k⭐) | Python | MIT | Layout NN + TableFormer, JSON lossless, **có audio ASR**, MCP | ~6.3s/trang CPU; OCR = Tesseract/EasyOCR (không tối ưu tiếng Việt) |
| **Marker/Datalab** (~37k⭐) | Python+API | GPL + **weights hạn chế thương mại** | 0.18s/trang (H100), Surya OCR 90+ lang (vi 73.2%) | License weights cấm công ty >$2M; GPU |
| **MinerU** (~73k⭐) | Python+API | Apache-based | **Đầu bảng OmniDocBench (95.75)**, VLM 1.2B, MCP | GPU 16GB+; tiếng Việt qua Paddle (mình đã đo Paddle kém vi) |
| **Firecrawl /parse** | Cloud API | AGPL core | Fire-PDF Rust + GPU OCR, <400ms/trang | Cloud-only, credit, không pptx/audio |
| **Unstructured** | Python+SaaS | Apache | 56 formats, connectors, VLM tier | $0.03/trang; không audio |
| **LlamaParse** | Cloud | đóng | 130+ formats **kể cả audio**, tier VLM | Cloud-only, $0.001–0.056/trang |
| **Kreuzberg v4** | **Rust core** | MIT | 90+ formats, multi-OCR, MCP, 11 bindings | tổng quát, không tối ưu vi |
| **ferrules** | **Rust** | — | pdfium + layout ML, Apple Vision OCR | macOS-centric |
| **Vibe** | **Tauri+Rust** whisper | OSS | tiền lệ đóng gói whisper-rs desktop 3 OS | chỉ audio |

## 2. Checklist "giải pháp tốt nhất" (xếp theo giá trị, đối chiếu mình)

| # | Pattern best-in-class | Mình |
|---|---|---|
| 1 | Trích PDF **có cấu trúc** (heading/bảng/đa cột) | ✅ pdf-inspector |
| 2 | Routing text-vs-OCR **theo trang** | ✅ |
| 3 | **MCP server** cho agent | ✅ (6 tool, hơn markitdown-mcp chỉ 1 tool) |
| 4 | Trích chọn lọc (pages/sheet/max_chars) — giảm token | ✅ (LlamaParse/Firecrawl chưa có page-range!) |
| 5 | Output **JSON lossless / schema extraction** | ⚠️ mới có tables_json + LLM extract_json |
| 6 | Tier **VLM cho tài liệu khó** (xu hướng thống trị 2026) | ⚠️ đã có khung LLM (cần key) — nên thêm vision |
| 7 | Audio ASR tích hợp | ✅ (chỉ Docling/LlamaParse có) |
| 8 | Chunking cho RAG | ❌ chưa |
| 9 | Plugin/extras system | ❌ chưa |
| 10 | Bảng → HTML khi bảng phức tạp (merge cell) | ❌ markdown-only |

## 3. Góc tiếng Việt — khoảng trống thị trường (từ nhánh research vi)

- **Không có sản phẩm nào** kết hợp: OCR tiếng Việt offline + PDF→MD có cấu trúc + ASR tiếng Việt, desktop. VietOCR (sourceforge) chỉ ảnh→text, đã cũ.
- Cloud vi (FPT.AI Read ~98%, Viettel OCR 99%*) đều **cloud-only** → Luật BVDLCN 91/2025 (hiệu lực 1/1/2026, phạt tới 5% doanh thu chuyển dữ liệu xuyên biên giới) là **gió thuận cho offline**.
- Cơ hội cụ thể:
  1. **PhoWhisper** (VinAI, WER vi ~6.3 small trên VIVOS, có bản ggml cộng đồng) — thay whisper thường = nâng accuracy audio vi rẻ nhất. *(kiểm tra license trước)*
  2. **Chuẩn hoá NFC + mapping TCVN3/VNI** cho tài liệu vi cũ — không đối thủ nào làm.
  3. **Vintern-1B** (VLM 1B tiếng Việt, on-device) — ứng viên tier vision offline.
  4. Hậu xử lý phục hồi dấu cho IN HOA.
  5. Công bố benchmark "văn bản hành chính" — chưa ai có.

## 4. Kết luận định vị

Mình = **"Docling offline thu nhỏ, tối ưu tiếng Việt, viết bằng Rust, có app desktop + MCP"**.
Giữ lợi thế: offline/riêng tư (PDPL), tiếng Việt đo được, audio, selective-extraction.
Nên bù: JSON/schema output đầy đủ, vision tier (Vintern-1B/cloud), PhoWhisper, RAG chunking.
