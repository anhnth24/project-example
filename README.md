# fileconv — convert mọi file sang Markdown (Rust)

Backend Rust chuyển đổi tài liệu/ảnh/âm thanh sang Markdown. Mục tiêu: hiệu năng
cao + tiếng Việt tốt, sau này đóng gói thành **desktop app (Tauri)** cho Win/Mac/Ubuntu.

> **Trạng thái: Phase 1 — backend + kiểm thử hiệu năng/độ chính xác.**
> Xem báo cáo đầy đủ ở [`bench/REPORT.md`](bench/REPORT.md).

## Cấu trúc

```
crates/core/        # fileconv-core: lõi convert — code của mình, gọi thẳng crate gốc
crates/cli/         # fileconv: bench harness (đo tốc độ + độ chính xác CER/WER)
bench/              # script tải corpus thật, sinh corpus tiếng Việt, các báo cáo
vendor/markitdown-rs/  # CHỈ tham khảo (không build, không phụ thuộc)
```

## Định dạng hỗ trợ (phase 1)

pdf, docx, pptx, xlsx/xls, csv, html + **ảnh OCR tiếng Việt** (Tesseract `vie+eng`)
+ **audio tiếng Việt** (whisper-rs + symphonia).
Mỗi định dạng gọi thẳng crate Rust gốc (calamine/docx-rust/pdf-extract/quick-xml/htmd/whisper-rs).

## Kết quả tóm tắt (Intel Xeon 2.8GHz, release)

- **Tốc độ**: pptx/csv/xlsx/docx < 1 ms/file; pdf ~5.7 ms/trang (PDFium); html ~15 ms/file.
- **Độ chính xác tiếng Việt**: docx/csv 100%, html 99%, **ảnh chữ in OCR ~99%** (tiền xử lý ảnh);
  tài liệu thật IN HOA dùng `tessdata_best` tốt hơn hẳn; chữ viết tay kém (giới hạn Tesseract).
- **Audio tiếng Việt** (whisper): tiny 86.8% / base 94.5% / small 97.0% độ chính xác;
  RTF 0.15 / 0.30 / 0.99 (nhỏ hơn 1 = nhanh hơn thời gian thực).

## Chạy thử

Yêu cầu: Rust, `tesseract-ocr` + `tesseract-ocr-vie`, `poppler-utils`, `imagemagick`,
`python3` (+ `python-docx python-pptx openpyxl` để sinh corpus).

```bash
# 1) Build
cargo build --release

# 1b) (PDF) tải thư viện PDFium — nếu thiếu sẽ tự fallback pdf-extract
bash bench/download_pdfium.sh

# 1c) (OCR) tải model tiếng Việt chất lượng cao — khuyến nghị cho tài liệu thật
bash bench/download_tessdata.sh

# 2) Convert một file
./target/release/fileconv one duong-dan/file.docx

# 3) Tải corpus thật & đo tốc độ
bash bench/download_corpus.sh
./target/release/fileconv speed bench/corpus bench/REPORT_SPEED.md

# 4) Sinh corpus tiếng Việt + ảnh & đo độ chính xác
python3 bench/make_vn_corpus.py
bash bench/make_vn_images.sh
./target/release/fileconv accuracy bench/vn_corpus/manifest.tsv bench/REPORT_ACCURACY.md

# 5) Audio tiếng Việt (whisper)
bash bench/download_models.sh
python3 bench/make_vn_audio.py
./target/release/fileconv audio models/ggml-base.bin bench/vn_audio/manifest.tsv bench/REPORT_AUDIO.md
```

## Đã sửa so với markitdown-rs (bản tham khảo)

Bản viết lại do mình làm chủ, đã khắc phục các lỗi phát hiện qua benchmark:
- `html2md` phình output (88M ký tự) → dùng `htmd`: nhỏ hơn ~90×, nhanh hơn ~7×.
- xlsx chỉ đọc sheet đầu → đọc tất cả sheet (+ hỗ trợ `.xls`).
- docx mất cấu trúc → phát hiện heading + bảng Markdown.
- pptx sai thứ tự slide + `println!` debug → sort đúng, output sạch.
- pdf-extract panic + trích thiếu → chuyển sang **PDFium** (nhanh 3×, đầy đủ hơn), fallback pdf-extract.
- docx dính chữ do `<w:br>`/`<w:tab>` → xử lý break/tab đúng.

Chi tiết & số liệu ở [`bench/REPORT.md`](bench/REPORT.md).
