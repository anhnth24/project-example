# fileconv — convert mọi file sang Markdown (Rust)

Backend Rust chuyển đổi tài liệu/ảnh/âm thanh sang Markdown. Mục tiêu: hiệu năng
cao + tiếng Việt tốt, sau này đóng gói thành **desktop app (Tauri)** cho Win/Mac/Ubuntu.

> **Trạng thái: Phase 1 — backend + kiểm thử hiệu năng/độ chính xác.**
> Xem báo cáo đầy đủ ở [`bench/REPORT.md`](bench/REPORT.md).

## Cấu trúc

```
crates/core/   # fileconv-core: lõi convert (bọc markitdown-rs + OCR Tesseract)
crates/cli/    # fileconv: bench harness (đo tốc độ + độ chính xác CER/WER)
bench/         # script tải corpus thật, sinh corpus tiếng Việt, các báo cáo
```

## Định dạng hỗ trợ (phase 1)

pdf, docx, pptx, xlsx, csv, html (qua markitdown-rs) + **ảnh OCR tiếng Việt** (Tesseract `vie+eng`).
Audio (whisper) ở phase sau.

## Kết quả tóm tắt (Intel Xeon 2.8GHz, release)

- **Tốc độ**: pptx/csv/xlsx/docx < 1 ms/file; pdf ~18 ms/trang; html theo kích thước DOM.
- **Độ chính xác tiếng Việt**: docx/csv 100%, html 99%, **ảnh chữ in OCR 98.5%**,
  ảnh scan kém 81%, chữ viết tay 33–62% (giới hạn của Tesseract).

## Chạy thử

Yêu cầu: Rust, `tesseract-ocr` + `tesseract-ocr-vie`, `poppler-utils`, `imagemagick`,
`python3` (+ `python-docx python-pptx openpyxl` để sinh corpus).

```bash
# 1) Build
cargo build --release

# 2) Convert một file
./target/release/fileconv one duong-dan/file.docx

# 3) Tải corpus thật & đo tốc độ
bash bench/download_corpus.sh
./target/release/fileconv speed bench/corpus bench/REPORT_SPEED.md

# 4) Sinh corpus tiếng Việt & đo độ chính xác
python3 bench/make_vn_corpus.py
#   (render ảnh: xem lệnh ImageMagick trong lịch sử/REPORT.md)
./target/release/fileconv accuracy bench/vn_corpus/manifest.tsv bench/REPORT_ACCURACY.md
```

## Hạn chế đã biết (markitdown-rs v0.1.11)

- `html2md` phình output bất thường ở trang lớn; xlsx chỉ đọc sheet đầu; docx mất
  cấu trúc heading; lib có `println!` debug; kéo dep LLM nặng. Chi tiết & hướng xử
  lý ở [`bench/REPORT.md`](bench/REPORT.md) §5–6.
