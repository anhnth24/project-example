# Báo cáo TEST — 10 bộ sample × 5 file (50 file)

> Corpus: `bench/sample10/` (tái tạo bằng `bench/make_sample10.sh`). Mọi file được
> kiểm tra magic-byte; **nguồn hỏng đã bị thay** (PhpSpreadsheet `sample.xls` trả JSON lỗi,
> file-examples `.xls` trả HTML → thay bằng file BIFF thật sinh từ xlwt, nội dung tiếng Việt).
> Máy: CPU 4 nhân, build release. Ngày: 02/07/2026.

## 1. Kết quả tổng: 50/50 convert thành công (100%), 0 crash

| # | Bộ | Số file | OK | ms/file (TB) | Ghi chú |
|---|---|--:|--:|--:|---|
| 1 | pdf (arXiv) | 5 | **5/5** | 252 (18.3 ms/trang, 69 trang) | pdf-inspector: heading + bảng |
| 2 | docx | 5 | **5/5** | 5.9 | bold/italic + bảng giữ đúng |
| 3 | pptx | 5 | **5/5** | 0.4 | đúng thứ tự slide |
| 4 | xlsx | 5 | **5/5** | 0.3¹ | đọc mọi sheet |
| 5 | xls legacy (BIFF) | 5 | **5/5** | 0.3¹ | calamine đọc cả file xlwt VN + đa sheet |
| 6 | csv | 5 | **5/5** | 0.3 | bảng markdown |
| 7 | html | 5 | **5/5** | 38 | có Wiki tiếng Việt lớn |
| 8 | ảnh PNG (VN) | 5 | **5/5** | ~390 | OCR tessdata_best |
| 9 | ảnh jpg/webp/bmp/tiff/gif | 5 | **5/5** | ~400 | mọi codec sau fix |
| 10 | audio mp3 (vi+en) | 5 | **5/5** | RTF 0.5 | whisper base |

¹ speed-bench gộp xls+xlsx thành nhóm "xlsx" (10 file, 10/10 OK).

## 2. Độ chính xác NỘI DUNG (có ground-truth)

**OCR ảnh tiếng Việt (10 ảnh, 2 bộ):**

| Bộ | Độ chính xác TB | CER | Chi tiết |
|---|--:|--:|---|
| image_png | **99.8%** | 0.002 | 4/5 ảnh đạt 100.0% |
| image_other (jpg/webp/bmp/tiff/gif) | **99.8%** | 0.002 | đồng nhất mọi định dạng |

**Audio (whisper base, 5 clip):**

| Clip | Độ chính xác | Ghi chú |
|---|--:|---|
| en_test | **100%** | tiếng Anh chuẩn |
| vn_camon | 96.4% | |
| vn_baocao | 88.5% | |
| vn_giaoban | 73.4% | "tám giờ ba mươi" vs số |
| vn_sodienthoai | 51.3% | đọc chuỗi số — ca khó cố hữu |
| **TB** | **81.9%** | RTF 0.50 (nhanh 2× realtime) |

## 3. File lỗi đã gặp & cách thay

| File hỏng | Triệu chứng | Thay bằng |
|---|---|---|
| PhpSpreadsheet `tests/.../sample.xls` | URL trả JSON error (195B) | `vn-*.xls` sinh bằng xlwt (BIFF thật, tiếng Việt) |
| file-examples `file_example_XLS_10.xls` | trả HTML (`3c21444f`) | `vn-dasheet.xls` (xlwt, 2 sheet) |

Validator trong `make_sample10.sh`: `%PDF` cho pdf, `PK` cho OOXML, `D0CF11E0` cho OLE/xls.

## 4. Kết luận

- **100% convert thành công** trên 50 file / 10 loại; không panic.
- OCR ảnh tiếng Việt ổn định **99.8%** ở mọi định dạng ảnh, ~0.4s/ảnh (sau fix upscale).
- Audio vi trung bình 81.9% với model base (số đọc thành lời vẫn là ca khó; xem
  PhoWhisper trong `RESEARCH_COMPETITORS.md` để nâng tiếp).
- Chi tiết từng file: `REPORT_SAMPLE10_SPEED.md`, `REPORT_SAMPLE10_IMG.md`, `REPORT_SAMPLE10_AUDIO.md`.
