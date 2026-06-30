# FileConv Docs (desktop)

App desktop (Tauri 2 + React/TS) cho **BA/PM** soạn tài liệu cho Dev: tải file gốc vào
thư mục, app gọi lõi Rust `fileconv-core` chuyển sang **Markdown** (link 1-1 với file gốc),
xem **song song** (gốc ↔ markdown) hoặc **sửa** markdown. **Toàn bộ dữ liệu lưu local.**

## Mô hình

- Một **thư mục gốc DATA** duy nhất. Mặc định `app_data_dir()/DATA`; có thể **map** sang
  thư mục bất kỳ của bạn (nút đổi thư mục trên sidebar; lưu ở `app_config_dir/config.json`).
- **Folder** = thư mục con thật trong DATA. **Document** = cặp `(file gốc, file .md)`.
- Quy ước link 1-1: `report.pdf` → `report.pdf.md` đặt cạnh nhau. Filesystem là nguồn sự thật.
- Định dạng nhận vào = đuôi mà `fileconv-core` hỗ trợ (pdf, docx, pptx, xlsx/xls/ods, csv,
  html, ảnh, audio). File không hỗ trợ sẽ bị chặn khi tải lên.

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

## Preview file gốc trong app (side-by-side với Markdown)

| Loại | Thư viện | Ghi chú |
|------|----------|---------|
| PDF | `pdfjs-dist` (pdf.js) | render từng trang ra canvas |
| Word `.docx` | `docx-preview` | giữ định dạng |
| Excel `.xlsx/.xls/.ods` | `@e965/xlsx` (SheetJS) | có tab chọn sheet |
| Ảnh, audio | asset protocol | `<img>` / `<audio>` |
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
- Chưa có drag-drop, đa tab, tìm kiếm, đóng gói cài đặt (Win/Mac/Ubuntu) — dự kiến sau.
