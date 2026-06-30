# Báo cáo Test Mở Rộng & Edge Case — fileconv-core

> Mở rộng corpus lên **15–20 file/loại** từ nhiều nguồn thật + bộ **edge case** chủ động,
> chạy backend để soi lỗi. Ngày: 2026-06-30. Build release.

## 1. Quy mô đã test

| Loại | Số file | Convert OK | Nguồn |
|---|--:|--:|---|
| pdf | 20 | **20/20** | arXiv (đa dạng độ dài/đề tài) |
| docx | 20 | **20/20** | python-docx + calibre |
| pptx | 20 | **20/20** | python-pptx |
| xlsx | 20 | **20/20** | PhpSpreadsheet templates |
| csv | 20 | **20/20** | burkardt datasets |
| html | 18 | **18/18** | web thật (Wiki vi/en/ja/ar, MDN, rust, gnu, python…) |
| image | 16 | **16/16** | tự render (font/cỡ/IN HOA/định dạng) + ảnh không chữ |
| audio | 10 | 10/10* | gTTS (vi/en) + wav/ogg thật |
| **edge** | 11 | xử lý đúng | rỗng/hỏng/sai-đuôi/unicode/legacy |

\* audio chạy bằng lệnh `audio` riêng (cần model whisper).

Tổng ~135 file. **Không có crash/panic nào** trong toàn bộ quá trình.

## 2. Bug THẬT phát hiện qua test mở rộng → ĐÃ SỬA

| # | Bug | Triệu chứng | Sửa |
|---|---|---|---|
| 1 | **HTML lọt `<script>`/`<style>`** | Trang thật (rust-lang.org) ra `window.RUST_BASE_URL=""`; trang chỉ có script ra `alert(1) p{color:red}` | `htmd` builder `skip_tags(["script","style","noscript"])` |
| 2 | **Ảnh non-PNG không được tiền xử lý** | crate `image` chỉ bật feature `png` → jpg/webp/bmp/tiff/gif rơi về OCR thô; **BMP nén lỗi hẳn** | Bật codec `jpeg/gif/bmp/webp/tiff` → mọi định dạng đều tiền xử lý + OCR (jpg/webp/bmp/tiff/gif giờ ra text giống PNG) |

## 3. Edge case — xử lý GRACEFUL (không sập)

| Tình huống | Kết quả |
|---|---|
| File **rỗng** (.pdf/.docx) | Báo lỗi rõ, không panic |
| File **rỗng** (.csv) | Ra rỗng (đúng) |
| **Rác/hỏng** (random bytes .pdf/.docx) | Báo lỗi rõ (Invalid header / EOCD), không panic |
| **Sai đuôi**: HTML đặt tên `.pdf`, text đặt tên `.docx` | Báo lỗi rõ (định tuyến theo đuôi) |
| **Mislabel** xlsx (LFS pointer / format khác) | Báo lỗi rõ, không panic → *đã thay bằng nguồn thật* |
| **legacy `.xls`** (BIFF thật) | **Đọc được** qua calamine ✓ |
| **CSV unicode** (dấu phẩy trong ô, xuống dòng trong ô, emoji 😀, CJK 日本語) | Parse đúng thành bảng ✓ |
| **HTML khổng lồ** (900 KB, 50.000 thẻ) | Xử lý xong, không phình ✓ |
| **Ảnh không có chữ** | OCR ra rỗng (đúng) ✓ |

## 4. Edge case AUDIO (whisper base)

- **Mọi định dạng decode OK** (mp3/wav/ogg qua symphonia) — không crash.
- Câu tiếng Việt thường: **88–94%** chính xác.
- ⚠️ **Số đọc thành lời** ("1234567890", "số năm tám") → điểm thấp (60–67%): lệch giữa
  chữ số trong ground-truth và chữ viết whisper xuất ra (một phần là do cách tính, không
  hẳn lỗi nhận dạng).
- ⚠️ **Audio không có tiếng nói (nhạc piano)** → whisper **"ảo giác"** sinh chữ vô nghĩa
  (lỗi cố hữu của whisper). Hướng xử lý: lọc theo `no_speech_probability`.
- ✓ Clip tiếng Anh dù ép `lang=vi` vẫn phiên âm đúng.

## 5. Hiệu năng trên corpus mở rộng (release)

| Loại | ms/file (TB) | ms/page (TB) | Ghi chú |
|---|--:|--:|---|
| pptx | 0.24 | 0.17 | |
| xlsx | 0.27 | — | đọc mọi sheet |
| docx | 1.16 | — | |
| csv | 2.01 | — | |
| html | 10.0 | — | có trang Wiki lớn |
| pdf | 100.0 | **5.66** | 353 trang, PDFium |
| image (OCR) | 484 | — | tessdata_best + tiền xử lý (chậm nhưng chính xác) |

## 6. Cải tiến tiềm năng (chưa phải bug)

- **Nhận diện theo magic-byte**: file sai đuôi hiện báo lỗi; có thể sniff nội dung để
  định tuyến đúng (zip→ooxml, %PDF→pdf, OLE→xls) và tự sửa đuôi sai.
- **Whisper no-speech filter**: bỏ đoạn `no_speech_prob` cao để tránh ảo giác trên
  audio không lời.
- **PDF/ảnh nhiều cột & IN HOA**: vẫn là ca khó của Tesseract (xem REPORT.md) — tier
  vision-LLM cho tài liệu khó.

## 7. Kết luận

Trên ~135 file thật đa dạng + 11 edge case cố ý: **backend không sập lần nào**, mọi lỗi
đều rõ ràng và đúng kỳ vọng. Hai bug thật (script HTML, codec ảnh) đã được phát hiện nhờ
mở rộng test và đã sửa + kiểm chứng. Sau khi thay các nguồn hỏng bằng file thật,
**tỉ lệ convert thành công của tài liệu + ảnh là 100%**.
