# Báo cáo PDF CASAN — OCR false-positive và hiệu năng

Ngày đo: 2026-07-10. File:
`Phương-pháp-luận-FPT-CASAN---ver.-Alpha_đã-ký 2.pdf` (45 trang, 1.61 MB,
Microsoft Word, tagged PDF). Build `--release`, máy sandbox 8 vCPU, PDFium local,
Tesseract `vie+eng` và `tessdata_best`.

## Chẩn đoán

PDF có text-layer Unicode tốt, không phải bản scan. `pdf-inspector` 0.1.3 gắn cờ
ba trang mục lục do font GID/symbol và dotted leader, khiến pipeline cũ render
rồi OCR Tesseract. OCR làm sai dấu/chữ dù text gốc hoàn toàn đọc được.

Một số bảng nhiều dòng/ô gộp cũng bị nhận thành quá nhiều cột Markdown, làm đảo
thứ tự câu. Header tài liệu lặp trên cả 45 trang.

File Markdown được nhắc trong commit test không có trên Git, nên phép đo nội dung
dùng `pdftotext -raw` làm tham chiếu text-layer.

## Thay đổi

- Ưu tiên native PDFium text khi qua cổng chất lượng cao; trang scan thật vẫn OCR.
- Page-filter dùng API lọc trang của `pdf-inspector`, không quét toàn bộ 45 trang.
- Bảng lỗi được cô lập theo trang và thay bằng native text nếu bảo toàn ≥90% nội dung.
- PDF 16–200 trang, ≤32 MB chạy tối đa 4 range song song trên máy ≥5 CPU.
- `pdf-inspector` và PDFium chạy chồng thời gian; PDFium binding tái sử dụng an toàn
  giữa các thread.
- Xóa header/footer lặp nhưng không đụng dòng bảng Markdown.
- Desktop dev tự tìm `pdfium/` và `tessdata_best/` từ các thư mục cha/executable,
  không phụ thuộc current working directory `app/`.
- Profile dev tối ưu core/dependency để `pnpm tauri dev` không chạy pipeline PDF
  hoàn toàn ở `opt-level=0`.

## Kết quả

| Kịch bản | Trước | Sau |
|---|---:|---:|
| Full 45 trang (CLI release) | ~0.77 s sau fix chất lượng tuần tự | **0.35 s** |
| Chọn 1 trang thường | ~0.40 s | **0.055 s** |
| Chọn 1 trang bảng | ~0.44 s | **0.059 s** |
| Chọn trang 1–10 | ~0.49 s | **0.27 s** |
| Desktop Tauri dev, click `Convert ngay` → `.md` | 17.54 s | **0.403 s** |
| Trang OCR nhầm | 3 | **0** |
| Header `Mã hiệu` lặp | 45 | **0** |
| Token recall (bỏ header lặp) | — | **99.86%** |
| Token precision | — | **100%** |

So với đường tuần tự bảo thủ, output song song giữ 99.95% token, đồng thời giữ
nhiều heading/bảng hợp lệ hơn. PDF ảnh-only sinh từ trang 4 vẫn đi qua
`<!-- Trang 1 (OCR) -->`.

Tauri được chạy thật trong Xvfb 1440×900 với DATA riêng: mở PDF raw, bấm
`Convert ngay`, sidecar 107.854 ký tự được tạo trong 0.403 giây; giao diện tự
chuyển sang chế độ Đối chiếu. Lần compile dev đầu tiên lâu hơn do dependency được
tối ưu, các lần chạy/compile tăng dần dùng incremental cache.

## Lệnh đo

```bash
cargo build --release -p fileconv-cli
./target/release/fileconv one "<file.pdf>"
./target/release/fileconv one "<file.pdf>" --pages 5
cargo test -p fileconv-core
cargo test -p fileconv-cli metrics
```

Hiệu năng phụ thuộc số core. Máy dưới 5 CPU, PDF >200 trang hoặc >32 MB tự dùng
đường tuần tự để tránh oversubscribe/OOM.
