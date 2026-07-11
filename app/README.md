# Markhand Desktop

App desktop (Tauri 2 + React/TS) cho **BA/PM** soạn tài liệu cho Dev: tải file gốc vào
thư mục, app gọi lõi Rust `fileconv-core` chuyển sang **Markdown** (link 1-1 với file gốc),
xem **song song**, **đối chiếu theo khối** hoặc sửa Markdown. **Toàn bộ dữ liệu lưu local.**

## Mô hình

- Một **thư mục gốc DATA** duy nhất. Mặc định `app_data_dir()/DATA`; có thể **map** sang
  thư mục bất kỳ của bạn (nút đổi thư mục trên sidebar; lưu ở `app_config_dir/config.json`).
- **Project** = một folder cấp trên trong DATA, có cây folder con độc lập. Sidebar chỉ
  hiện tài liệu thuộc project đang chọn.
- Có thể tạo project rỗng, tạo folder con rồi upload file; hoặc **Import folder local**
  để copy nguyên cây thư mục vào project và tự queue convert các file được hỗ trợ.
- Markhand không tạo symlink tới folder ngoài DATA; cách copy này giữ path-jail,
  đóng gói và xóa project an toàn.
- **Folder** = thư mục con thật trong DATA. **Document** = cặp `(file gốc, file .md)`.
- Quy ước link 1-1: `report.pdf` → `report.pdf.md` đặt cạnh nhau. Filesystem là nguồn sự thật.
- Định dạng nhận vào = đuôi mà `fileconv-core` hỗ trợ (pdf, docx, pptx, xlsx/xls/ods, csv,
  html, ảnh, audio). File không hỗ trợ sẽ bị chặn khi tải lên.

## Chạy dev

```bash
cd app
pnpm install
pnpm tauri dev      # mở app; tự chạy Vite (cổng 1420) + biên dịch backend Rust
```

Chỉ build phần web: `pnpm build` (ra `app/dist`). Unit test frontend: `pnpm test`.
Chỉ build backend: `cargo build -p fileconv-desktop` (từ thư mục gốc repo).

## Yêu cầu hệ thống

- **Rust** + **Node 20+** + pnpm 10.
- **Linux**: `webkit2gtk-4.1`, `libgtk-3`, (tùy chọn) `libayatana-appindicator3`, `librsvg2`.
- Build `fileconv-core` cần **cmake + clang** (biên dịch whisper.cpp lần đầu ~1–2 phút).
- Tùy chọn để convert đầy đủ (xem `CLAUDE.md` ở gốc repo): `tesseract-ocr(+vie)`, libpdfium,
  model whisper (mục Cài đặt trong app để trỏ tới `ggml-*.bin`).

## Cấu trúc

```
app/
  src/                 # React + TS
    lib/               # invoke(), types, tree + Markdown block helpers
    state/store.ts     # Zustand: DATA tree, tabs, draft và convert queue
    components/        # rail/drawer/tabs/library/workbench/compare/settings
  src-tauri/
    src/lib.rs         # các #[tauri::command] + thao tác filesystem an toàn
    tauri.conf.json    # cấu hình cửa sổ, CSP và bundle
```

## Luồng làm việc

- Icon rail mở Trang chủ, Thư viện, drawer tài liệu, tìm kiếm `Ctrl/Cmd+K`, hàng đợi và
  cài đặt.
- Mỗi tài liệu mở trong một tab riêng; draft chưa lưu được giữ khi chuyển tab.
- Upload/kéo-thả copy file vào DATA trước, sau đó convert tuần tự ở background. Queue chỉ
  hiển thị trạng thái thật (`đợi/chạy/xong/lỗi`), không dựng phần trăm giả.
- Bốn chế độ tài liệu: **Đối chiếu**, **Song song**, **Markdown**, **File gốc**.
- Đối chiếu khối dùng snapshot Markdown của lần convert gần nhất ở bên trái và draft đang
  sửa ở bên phải. File nguồn thật luôn xem được trong chế độ Song song/File gốc.
- `Ctrl/Cmd+S` lưu, `Ctrl/Cmd+W` đóng tab có xác nhận nếu còn draft.

## Document Intelligence / Bàn giao

Nút ✨ trên icon rail mở workspace Intelligence:

- Sinh BRD/PRD, user stories, acceptance criteria, glossary, test cases và
  traceability có citation.
- Baseline chạy offline; tùy chọn LLM dùng `FILECONV_LLM_*`.
- Quality report, SQLite FTS5 + vector search và hỏi đáp corpus có trích dẫn.
- Snapshot phiên bản, diff và merge an toàn trước reconvert.
- Sửa bảng Markdown, trích schema và xuất CSV.
- Quét/che PII, watch-folder rules và Knowledge Pack ZIP.
- Artifacts được lưu dưới `DATA/.markhand/`; Markdown canonical cạnh file nguồn
  không bị thay đổi trừ khi người dùng bấm lưu rõ ràng.

### LLM providers

Trong **Cài đặt → Document Intelligence**, Markhand có preset:

- Local/self-host: Ollama, LM Studio, llama.cpp server, vLLM.
- Cloud: OpenAI, Anthropic, Gemini, OpenRouter, Groq, Mistral AI, Together AI.
- Subscription: Cursor Agent CLI và OpenAI Codex CLI qua browser login chính thức.
- Custom OpenAI-compatible endpoint.

Mặc định LLM tắt; hybrid search, Q&A extractive và BRD/PRD deterministic vẫn
chạy offline. Index incremental nằm ở `DATA/.markhand/knowledge.sqlite`. Nếu đã
cấu hình nhưng provider không chạy, mất mạng hoặc thiếu key, Q&A tự fallback sang
extractive có citation và hiển thị cảnh báo; kết quả retrieval không mất.
Khuyến nghị Ollama/local để dữ liệu không rời máy:

```bash
ollama serve
ollama pull qwen2.5:7b
```

API key nhập trong desktop chỉ giữ trong memory, không ghi vào `settings.json`.
Muốn persist qua lần khởi động, đặt `FILECONV_LLM_API_KEY` trong environment.
Cloud/LLM chỉ nhận các citation đã retrieval, không nhận toàn bộ DATA root.

Neural embeddings là cấu hình riêng: Ollama/LM Studio/vLLM/OpenAI/Gemini. Nếu
tắt, index dùng local hash 256D. Nếu bật cloud embedding, toàn bộ chunk được gửi
khi build index (UI cảnh báo rõ), không chỉ top-K.

### Build bộ cài

```bash
cd app
pnpm tauri build --bundles deb       # Linux
# CI release matrix: deb/AppImage, MSI/NSIS và DMG
```

`.deb` Linux đã được build và kiểm tra metadata; artifact nằm dưới
`target/release/bundle/`. Windows/macOS cần runner đúng hệ điều hành và
signing/notarization trước khi phát hành công khai.

## Preview file gốc trong app

| Loại | Thư viện | Ghi chú |
|------|----------|---------|
| PDF | `pdfjs-dist` (pdf.js) | render từng trang ra canvas |
| Word `.docx` | `docx-preview` | giữ định dạng |
| Excel `.xlsx/.xls/.ods` | `@e965/xlsx` (SheetJS) | có tab chọn sheet |
| Ảnh, audio | Blob URL từ `read_bytes` | `<img>` / `<audio>` |
| csv, html, text, markdown | đọc trực tiếp | hiển thị thô |
| `.pptx` | — | webview render chưa đáng tin → nút **Mở ngoài** |

Bytes file đọc qua command Rust `read_bytes` (trả ArrayBuffer); ảnh/audio dựng `Blob`
URL từ bytes đó. KHÔNG dùng `fetch(asset://)`/`<img src=asset://>` vì webview chặn (403).

### File quá khổ

- Text/CSV/log: chỉ đọc 512KB đầu (`read_text_preview`) + banner "Mở ngoài để xem đầy đủ".
- PDF: render **lazy** từng trang theo viewport (IntersectionObserver) → mở file nhiều trang vẫn nhẹ.
- Excel: tối đa 1000 dòng/sheet khi preview + báo số dòng thật.
- Trước khi render, kiểm tra kích thước (`file_size`); vượt ngưỡng (pdf 80MB, docx/excel 40MB,
  ảnh 60MB, audio 120MB) sẽ hỏi "Vẫn xem trong app / Mở ngoài".

## Hạn chế đã biết

- `.pptx`: chưa preview trong app (dùng "Mở ngoài").
- Queue chưa có phần trăm theo page/segment vì `fileconv-core` chưa phát progress unit.
- Đối chiếu hiện liên kết bản convert gốc ↔ draft Markdown; source-anchor theo page/slide/
  sheet/timestamp cần structured converter artifact ở phase sau.
- Linux `.deb` đã xác minh; AppImage/Windows/macOS chưa smoke-test artifact hoặc ký số.
