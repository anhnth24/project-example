# [intern-28] Input validation & security — converter robustness test

Issue: [#366](https://github.com/anhnth24/project-example/issues/366)

## Malicious files (`/tmp/fileconv-intern28/`)

Crafted via `craft_malicious.py` (local only — not committed).

| File | Loại | Mô tả cấu trúc |
|------|------|----------------|
| `oversized.docx` | Oversized ZIP | OOXML + 10×512KiB STORED media entries (~5MiB uncompressed) |
| `nested-3.docx` | Nested ZIP | zip→zip→zip nhúng trong `word/embeddings/nested.zip` |
| `traversal-evil.docx` | Zip Slip | entry `../../../tmp/evil.txt` |
| `truncated.docx` | Corrupt DOCX | OOXML cắt còn 120B, mất EOCD |
| `truncated.pdf` | Corrupt PDF | PDF cắt xref trailer |
| `local-bomb.docx` | Compression bomb | 1MiB zeros, DEFLATE ratio cao |

## Kết quả `fileconv one`

Environment: macOS, `./target/release/fileconv`, 2026-08-12.

| File | Exit | Time | Output |
|------|------|------|--------|
| `local-bomb.docx` | 0 | 0.585s | `synthetic` |
| `nested-3.docx` | 0 | 0.010s | `synthetic` |
| `oversized.docx` | 0 | 0.009s | `synthetic` |
| `traversal-evil.docx` | 0 | 0.006s | `synthetic` |
| `truncated.docx` | 1 | 0.005s | `Zip(InvalidArchive("Could not find EOCD"))` |
| `truncated.pdf` | 1 | 0.011s | `Invalid cross-reference table (invalid start value)` |

Không crash, không hang trên mọi case.

## Limit nào được check?

Core CLI (`fileconv one`) **không check** file size, zip-slip, nested archive, hay compression ratio cho DOCX.
Chỉ fail khi structure hỏng (`truncated.docx` / `truncated.pdf`).

Production limits nằm ở server upload ([`crates/server/src/services/upload/archive.rs`](../../crates/server/src/services/upload/archive.rs)).

**Security limit có trong core** (không áp case DOCX này): image OCR `MAX_DECODE_SIDE=12000`, `image::Limits` 512MiB ([`crates/core/src/image_ocr.rs`](../../crates/core/src/image_ocr.rs)).

## Timeout bao lâu?

- **Core:** không có wall-clock timeout — chạy đến khi xong hoặc lỗi.
- **Test harness:** wrapper `timeout 30s` (không case nào chạm).
- **Server sandbox policy:** 420s ([upload policy](../markhand-web-upload-policy.md) §2).

## Reject sao?

- **exit 1** + stderr `Error: chuyển đổi thất bại: ...` — truncated/corrupt cases.
- **exit 0** + markdown `synthetic` — malicious ZIP entries ignored khi `word/document.xml` hợp lệ.
- Không phân biệt security reject vs parse error — cùng variant `ConvertError::Failed`.

## DoS protection quan sát được

PDF `catch_unwind` quanh pdf-extract ([`crates/core/src/conv/pdf/fallback.rs`](../../crates/core/src/conv/pdf/fallback.rs)) — corrupt PDF không crash process.

## Finding chính

Core converter **fail-soft** (không crash/hang) nhưng **không phải security boundary** cho archive attacks: traversal/bomb/nested/oversized entries bị bỏ qua nếu OOXML skeleton hợp lệ.

## Đề xuất improvement (optional implement)

Thêm archive preflight scan (entry names + compression ratio + nested magic) trước OOXML parse, reuse logic từ `archive.rs`. Hiện core trả exit 0 thay vì reject sớm (exit 1) cho traversal/bomb.
