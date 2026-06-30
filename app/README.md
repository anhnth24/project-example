# FileConv Docs (desktop)

App desktop (Tauri 2 + React/TS) cho **BA/PM** soạn tài liệu cho Dev: đưa file gốc vào
workspace, app gọi lõi Rust `fileconv-core` chuyển sang **Markdown** (link 1-1 với file gốc),
xem **song song** (gốc ↔ markdown) hoặc **sửa** markdown. **Toàn bộ dữ liệu lưu local.**

## Mô hình

- **Workspace** = một thư mục thật trên đĩa bạn chọn (danh sách lưu ở `app_config_dir`).
- **Folder** = thư mục con thật. **Document** = cặp `(file gốc, file .md)`.
- Quy ước link 1-1: `report.pdf` → `report.pdf.md` đặt cạnh nhau. Filesystem là nguồn sự thật.
- Định dạng nhận vào = đuôi mà `fileconv-core` hỗ trợ (pdf, docx, pptx, xlsx/xls/ods, csv,
  html, ảnh, audio). File không hỗ trợ sẽ bị chặn khi import.

## Chạy dev

```bash
cd app
npm install
npm run tauri dev      # mở app; tự chạy Vite (cổng 1420) + biên dịch backend Rust
```

Chỉ build phần web: `npm run build` (ra `app/dist`).
Chỉ build backend: `cargo build -p fileconv-desktop` (từ thư mục gốc repo).

## Yêu cầu hệ thống

- **Rust** + **Node 18+**.
- **Linux**: `webkit2gtk-4.1`, `libgtk-3`, (tùy chọn) `libayatana-appindicator3`, `librsvg2`.
- Build `fileconv-core` cần **cmake + clang** (biên dịch whisper.cpp lần đầu ~1–2 phút).
- Tùy chọn để convert đầy đủ (xem `CLAUDE.md` ở gốc repo): `tesseract-ocr(+vie)`, libpdfium,
  model whisper (mục Cài đặt trong app để trỏ tới `ggml-*.bin`).

## Cấu trúc

```
app/
  src/                 # React + TS
    lib/{ipc,types}.ts # cầu nối invoke() + kiểu dữ liệu
    state/store.ts     # zustand: workspace, cây, lựa chọn
    components/        # Sidebar, Tree, DocView, SourcePreview, MarkdownEditor, Settings
  src-tauri/
    src/lib.rs         # các #[tauri::command] + thao tác filesystem an toàn
    tauri.conf.json    # cấu hình app + asset protocol cho preview
```

## Hạn chế đã biết (giai đoạn 1)

- Preview file gốc native chỉ cho **ảnh / audio / text/csv/html / pdf** (PDF tùy webview OS).
  **docx/pptx/xlsx** không xem trước trong app → nút "Mở ngoài" + đối chiếu bản Markdown.
- Chưa có drag-drop, đa tab, tìm kiếm, đóng gói cài đặt (Win/Mac/Ubuntu) — dự kiến giai đoạn 2.
