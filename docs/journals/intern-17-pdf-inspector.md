# intern-17: Hiểu PDF-inspector (structured PDF + multi-column)

**Issue:** [#355](https://github.com/anhnth24/project-example/issues/355)
**Date:** 2026-08-03
**Component:** `fileconv-core` PDF path (`crates/core/src/conv/pdf/`)
**Status:** Learning notes only — no convert logic changed

## Môi trường

- OS: macOS arm64
- Build: `cargo build --release -p fileconv-cli` (tránh `--workspace` — `fileconv-server` sandbox dùng Linux-only APIs)
- PDFium: có — `bash bench/download_pdfium.sh mac-arm64` → `pdfium/lib/libpdfium.dylib` (153.0.7988.0)
- Lệnh: `./target/release/fileconv one <file.pdf>`

## Flow 3-tier (tóm tắt)

```
.pdf → Converter::convert_path → FormatKind::Pdf
  → to_markdown_detailed (crates/core/src/conv/pdf/mod.rs)
      1) pdf-inspector: MD có cấu trúc (heading / bảng / sắp đa cột) + cờ needs_ocr
      2) via_pdfium: khi inspector Unavailable / Abandoned / MD rỗng
      3) pdf-extract: khi PDFium cũng fail và không filter pages
```

- **pdf-inspector:** chữ + toạ độ + cỡ font → `#` heading, đọc đa cột trái→phải, bảng Markdown; gắn `needs_ocr` nếu scan hoặc text-layer rác (font GID…).
- **Trang `needs_ocr`:** ưu tiên text PDFium tin cậy → OCR render 300 DPI + Tesseract → giữ untrusted + warning → unresolved thì Abandoned cả đường inspector.
- **Rơi PDFium khi:** inspector Unavailable (panic/lỗi) | Abandoned | Success nhưng markdown rỗng.
- **Rơi pdf-extract khi:** PDFium `None`/rỗng **và** không chọn `pages`. Có filter pages + fail → lỗi, không gọi extract.

> Issue vẫn ghi `conv/pdf.rs`; code đã tách thành module `conv/pdf/` (`mod.rs`, `inspector.rs`, `recovery.rs`, `fallback.rs`, …).

## Font-size → heading level

Trong pdf-inspector: body size ≈ cỡ chữ phổ biến trên trang; cỡ lớn hơn body (~≥1.2×) được xếp tier → Markdown `#` … `####`.

Chứng minh bằng `gold-001.pdf`:

```
# Hồ sơ quy trình mua sắm số 01

## Mã hồ sơ là HS-2026-001.

Ngân sách được phê duyệt là 137 triệu đồng.
```

Tiêu đề lớn → `#`; dòng phụ → `##`; đoạn body không có prefix heading.

## PDF-inspector phát hiện gì? Khi nào fallback?

| Thứ | pdf-inspector | Fallback PDFium / pdf-extract |
|-----|---------------|-------------------------------|
| Heading | cỡ font → `#` / `##` … | text phẳng, mất hierarchy |
| Đa cột | sắp lại trái → phải | dễ đọc ngang lẫn cột |
| Bảng | → Markdown table | thường mất lưới |
| `needs_ocr` | cờ trang scan / text-layer rác | OCR (cần PDFium) hoặc giữ untrusted / extract |

## Case 1 — inspector tốt hơn pdf-extract

**File:** `bench/markhand_web/golden/documents/gold-001.pdf`

```bash
./target/release/fileconv one bench/markhand_web/golden/documents/gold-001.pdf
```

Output có `#` / `##` và tiếng Việt đúng. pdf-extract thường chỉ trả một cục text phẳng, không giữ hierarchy heading như vậy.

## Case 2 — cần OCR (trước / sau PDFium)

**File:** `tests/fixtures/pdf/needs_ocr_untrusted_fallback.pdf`

| Điều kiện | Output (rút gọn) | Ý nghĩa |
|-----------|------------------|---------|
| Không PDFium | `!!!@@@###$$$%%%…""ab` (không marker OCR) | Text-layer rác; không render được → giữ/extract untrusted |
| Có PDFium | `<!-- Trang 1 (OCR) -->` rồi chữ OCR kém | `needs_ocr` → render 300 DPI + Tesseract |

Sau khi có `libpdfium.dylib`:

```
<!-- Trang 1 (OCR) -->

II@@(@##S$$9%%%^^^&&&***#_— +++===~~~[[[I((0;;:::222,,...."ab
```

Marker `<!-- Trang 1 (OCR) -->` chứng minh hệ thống **không tin text-layer**, chuyển sang OCR. Nội dung OCR vẫn kém vì fixture cố ý rác/khó — đủ cho AC “một trường hợp cần OCR”.

## Acceptance criteria

- [x] Convert thành công ≥2 PDF (`gold-001`, `needs_ocr_untrusted_fallback`)
- [x] Giải thích font-size → heading level (mục trên + output gold-001)
- [x] 1 case inspector > extract + 1 case cần OCR (bảng trước/sau PDFium)
- [x] Không sửa logic pdf-inspector / convert PDF
