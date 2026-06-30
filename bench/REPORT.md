# Báo cáo Backend `fileconv-core` — Tốc độ & Độ chính xác

> Phase 1: backend chuyển đổi file → Markdown, viết bằng **Rust**, kiểm thử trên
> **dữ liệu thật tải từ Internet** + corpus **ground-truth tiếng Việt**.
> Ngày chạy: 2026-06-30.

## 1. Backend là gì

`fileconv-core` là crate Rust bọc **`markitdown-rs`** (crate `markitdown` v0.1.11 —
đây là **bản viết lại hoàn toàn bằng Rust** của Microsoft MarkItDown, *không* nhúng
Python). Bên dưới dùng các crate Rust thuần:

| Định dạng | Crate Rust | Cơ chế |
|---|---|---|
| pdf | `pdf-extract` | trích text theo layout |
| docx | `docx-rust` | parse OOXML, lấy paragraph/table |
| pptx | `zip` + `quick-xml` | đọc `ppt/slides/*.xml` |
| xlsx | `calamine` | đọc workbook |
| csv | `csv` | đọc record |
| html | `html2md` | DOM → Markdown |
| **ảnh (OCR)** | **Tesseract** (`vie+eng`) | tự thêm — markitdown gốc chỉ đọc EXIF |

Toàn bộ pipeline biên dịch thành **binary Rust native** (release). Không có Python
lúc chạy convert (python3 chỉ dùng trong *bench harness* để đếm slide pptx).

## 2. Môi trường & phương pháp

- Máy: Intel Xeon @ 2.80GHz, 4 nhân; Linux; build `--release` (optimized).
- **Tốc độ**: 60 file thật (10/loại) tải từ arXiv, python-docx, python-pptx,
  PHPOffice/PhpSpreadsheet, burkardt CSV, và web thật (gồm Wikipedia tiếng Việt).
  Tổng 26 MB. Đo bằng `Instant`, mỗi file convert 1 lần.
- **Độ chính xác**: corpus tiếng Việt tự sinh, cùng một đoạn tham chiếu (399 ký tự,
  giàu dấu thanh) nhúng vào docx/pptx/xlsx/csv/html, và render thành ảnh cho OCR.
  Chỉ số: **CER/WER** (khoảng cách Levenshtein, chuẩn hoá khoảng trắng),
  Độ chính xác = (1 − CER)×100.

Tái lập:
```bash
bash bench/download_corpus.sh                 # tải 60 file thật
python3 bench/make_vn_corpus.py               # sinh corpus tiếng Việt
# (+ render ảnh bằng ImageMagick, xem lệnh trong README)
cargo build --release -p fileconv-cli
./target/release/fileconv speed    bench/corpus              bench/REPORT_SPEED.md
./target/release/fileconv accuracy bench/vn_corpus/manifest.tsv bench/REPORT_ACCURACY.md
```

## 3. Kết quả TỐC ĐỘ (release, 10 file/loại)

| Loại | Số file | OK | Σ KB | Σ pages | ms/file (TB) | ms/page (TB) | KB/s (TB) |
|---|--:|--:|--:|--:|--:|--:|--:|
| pptx | 10 | 10 | 562 | 17 | **0.20** | **0.12** | 277 277 |
| csv  | 10 | 10 | 165 | — | **0.27** | — | 60 421 |
| xlsx | 10 | 10 | 359 | — | **0.27** | — | 134 763 |
| docx | 10 | 10 | 188 | — | **0.84** | — | 22 290 |
| html | 10 | 10 | 3 840 | — | **131.9** | — | 2 912 |
| pdf  | 10 | 10 | 20 539 | 139 | **254.1** | **18.3** | 8 082 |

**100% file convert thành công.** Nhận xét:
- **pptx/csv/xlsx/docx**: dưới ~1 ms/file — cực nhanh (chỉ parse cấu trúc).
- **pdf** là nặng nhất: ~**18 ms/trang** (`pdf-extract` dựng lại layout text). File 22
  trang (`arxiv-2010.11929`) mất ~267 ms; bài dài 12 trang nhiều hình mất tới 639 ms.
- **html** phụ thuộc kích thước DOM: `example.html` 1 ms, nhưng `wiki-vietnam-vi`
  (2.2 MB) mất ~1 036 ms.

## 4. Kết quả ĐỘ CHÍNH XÁC (tiếng Việt)

| Kịch bản | Ref | Hyp | CER | WER | Độ chính xác | Ghi chú |
|---|--:|--:|--:|--:|--:|---|
| docx text | 399 | 399 | 0.000 | 0.000 | **100.0%** | hoàn hảo |
| csv text  | 399 | 399 | 0.000 | 0.000 | **100.0%** | hoàn hảo |
| html text | 399 | 402 | 0.008 | 0.011 | **99.2%** | gần hoàn hảo |
| **ảnh chữ in (OCR)** | 399 | 399 | 0.015 | 0.067 | **98.5%** | Tesseract `vie` rất tốt |
| pptx text | 399 | 419 | 0.075 | 0.169 | **92.5%** | *trừ điểm do markup, xem dưới* |
| xlsx text | 399 | 431 | 0.080 | 0.169 | **92.0%** | *trừ điểm do markup bảng* |
| ảnh scan kém (OCR) | 399 | 333 | 0.190 | 0.270 | **81.0%** | phân giải thấp |
| ảnh "viết tay" (OCR) | 399 | — | 0.521 | 0.888 | **47.9%** | *mô phỏng bằng font, xem dưới* |

### Diễn giải quan trọng
- **Tiếng Việt giữ dấu hoàn hảo** ở docx/csv/html và ~98.5% ở ảnh chữ in. Diacritics
  (sắc/huyền/hỏi/ngã/nặng, ơ/ư/ă/đ) không bị mất.
- **pptx (92.5%) và xlsx (92.0%) KHÔNG phải lỗi chữ Việt** — phần chênh là **markup
  cấu trúc** backend chèn thêm: pptx thêm `<!-- Slide number: 1 -->` (~20 ký tự),
  xlsx render thành bảng Markdown (dấu `|`, `---`). Nội dung chữ Việt vẫn đúng 100%.
- **OCR chữ in tiếng Việt: 98.5%** — đạt mức dùng được thực tế. Khi ảnh phân giải
  thấp tụt còn **81%**.
- **OCR "chữ viết tay": 33–62%** (mô phỏng bằng font Caveat/Dancing Script, *không
  phải chữ viết tay người thật*). Đây là giới hạn đã biết: **Tesseract không được
  thiết kế cho chữ viết tay**. Muốn OCR chữ viết tay tiếng Việt tốt cần engine khác
  (vd PaddleOCR, TrOCR, hoặc vision-LLM) — xem khuyến nghị.

## 5. Phát hiện & hạn chế của markitdown-rs (cần xử lý ở phase sau)

1. **`html2md` phình output bất thường**: `wiki-rust.html` (579 KB) sinh ~**20 triệu**
   ký tự; `wiki-vietnam-vi.html` (2.2 MB) sinh ~**88 triệu** ký tự — gần như chắc chắn
   là lỗi đệ quy/bảng lồng. Cần thay parser HTML (vd `htmd`, hoặc `scraper`+tự viết).
2. **xlsx chỉ đọc sheet đầu tiên** và hardcode kiểu `Xlsx` → **`.xls` cũ sẽ lỗi**.
   Cần lặp mọi sheet và hỗ trợ `Xls`.
3. **docx mất cấu trúc**: chỉ nối text, **không** xuất heading/bullet/bảng Markdown
   đúng nghĩa (nhiều file test ra rất ít ký tự). Cần map style → Markdown.
4. **ảnh**: bản gốc chỉ đọc EXIF, **không OCR** — đã thay bằng Tesseract trong core.
5. **`println!` debug rải rác** trong lib (excel "Opening file", pptx "slide:",
   image "markdown:") làm bẩn stdout. Cần fork bỏ đi.
6. **Kéo dependency nặng** (`rig-core`, `tokio`, `reqwest`…) cho nhánh LLM mà ta không
   dùng → build lâu (~57s core). Nên fork để cắt bớt.

## 6. Khuyến nghị cho phase tiếp theo

- **Fork markitdown-rs** (hoặc gọi thẳng `calamine`/`docx-rust`/… ) để: sửa html2md,
  đọc đủ sheet xlsx, giữ cấu trúc docx, bỏ `println!`, cắt dep LLM.
- **OCR**: giữ Tesseract `vie` cho chữ in (98.5% là tốt). Với **chữ viết tay** và ảnh
  khó, bổ sung tuỳ chọn engine mạnh hơn (PaddleOCR/vision-LLM) — đúng nhu cầu tiếng Việt.
- **Audio (whisper-rs)**: chưa làm ở phase này; đo WER tiếng Việt theo cỡ model ở phase sau.
- **PDF**: 18 ms/trang chấp nhận được; nếu cần nhanh hơn cân nhắc `pdfium-render`.

## 7. Tệp liên quan
- `bench/REPORT_SPEED.md` — bảng tốc độ chi tiết từng file.
- `bench/REPORT_ACCURACY.md` — bảng độ chính xác chi tiết.
- `bench/corpus/` — 60 file thật. `bench/vn_corpus/` — corpus tiếng Việt + ảnh.
- `crates/core/` — backend. `crates/cli/` — bench harness.
