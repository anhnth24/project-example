# intern-23 (#361) — Error handling test log

**Date:** 2026-08-07  
**Commit:** `d56321d8`  
**Binary:** `./target/release/fileconv one-detailed`  
**Focus:** PDF fallback chain + DOCX fail-fast contrast  
**PDFium:** yes | **Tesseract:** 5.5.0

## Summary table

| Case | File | Intent | Exit | kind / outcome |
|------|------|--------|-----:|----------------|
| 1 | `/tmp/intern361/ok.txt` | OK txt | 0 | `full_success` |
| 2 | `/tmp/intern361/ok.pdf` (soak-pdf) | OK PDF | 0 | `full_success` |
| 3 | `/tmp/intern361/corrupt-trunc.pdf` | corrupt PDF | 1 | `failed` |
| 4 | `/tmp/intern361/fake.pdf` | mismatch (.txt→.pdf) | 1 | `failed` |
| 5 | `/tmp/intern361/malformed.docx` | corrupt DOCX | 1 | `failed` |
| 6 | `tests/fixtures/pdf/needs_ocr_untrusted_fallback.pdf --no-pdf-ocr` | partial (best-effort) | 0 | `partial_success` + warning |

---

## Case 1: OK txt

- **File:** `/tmp/intern361/ok.txt` (copy of `tests/fixtures/sample/document.txt`)
- **Intent:** OK baseline — text parser
- **Command:** `./target/release/fileconv one-detailed /tmp/intern361/ok.txt`
- **Exit code:** 0
- **Outcome:** `full_success`, no warnings

```json
{
  "format": "text",
  "markdown": "Tài liệu kiểm thử tổng hợp cho Markhand.\nNội dung không chứa dữ liệu khách hàng hoặc thông tin định danh.\n",
  "outcome": "full_success",
  "title": "ok",
  "warnings": []
}
```

**Interpretation:** Text path is simplest — read file, normalize NFC, no fallback chain. Fail-fast only on I/O errors.

---

## Case 2: OK PDF

- **File:** `/tmp/intern361/ok.pdf` (`bench/markhand_web/soak/fixtures/soak-pdf.pdf`)
- **Intent:** OK PDF — native text layer
- **Command:** `./target/release/fileconv one-detailed /tmp/intern361/ok.pdf`
- **Exit code:** 0
- **Outcome:** `full_success`, no warnings

```json
{
  "format": "pdf",
  "markdown": "SOAKPDF15",
  "outcome": "full_success",
  "title": "ok",
  "warnings": []
}
```

**Interpretation:** Valid PDF with trustworthy native text — pdf-inspector (or fast path) succeeds without needing OCR or untrusted fallback.

---

## Case 3: Corrupt PDF

- **File:** `/tmp/intern361/corrupt-trunc.pdf` (truncated 64 bytes from `needs_ocr_untrusted_fallback.pdf`)
- **Intent:** corrupt — broken xref
- **Command:** `./target/release/fileconv one-detailed /tmp/intern361/corrupt-trunc.pdf`
- **Exit code:** 1
- **ConvertErrorKind:** `failed`

```json
{
  "message": "chuyển đổi thất bại: PDF error: Invalid cross-reference table (invalid start value)",
  "kind": "failed"
}
```

**Interpretation:** All PDF fallbacks fail on structurally invalid PDF → **fail-fast** hard error after inspector/PDFium/pdf-extract chain exhausts. No partial markdown returned.

---

## Case 4: Format mismatch (extension spoof)

- **File:** `/tmp/intern361/fake.pdf` (plain `.txt` content, renamed to `.pdf`)
- **Intent:** mismatch — router sends to PDF pipeline by extension only
- **Command:** `./target/release/fileconv one-detailed /tmp/intern361/fake.pdf`
- **Exit code:** 1
- **ConvertErrorKind:** `failed`

```json
{
  "message": "chuyển đổi thất bại: PDF error: Invalid file header",
  "kind": "failed"
}
```

**Interpretation:** `FormatKind::from_path` uses extension, not magic bytes — `.pdf` routes to PDF converter, which rejects invalid header. **Fail-fast** with clear message; no silent mis-conversion.

---

## Case 5: Malformed DOCX (fail-fast contrast)

- **File:** `/tmp/intern361/malformed.docx` (invalid ZIP header `PK\x03\x04not-a-valid-ooxml`)
- **Intent:** corrupt OOXML — DOCX has **no fallback chain**
- **Command:** `./target/release/fileconv one-detailed /tmp/intern361/malformed.docx`
- **Exit code:** 1
- **ConvertErrorKind:** `failed`

```json
{
  "message": "chuyển đổi thất bại: Zip(InvalidArchive(\"Could not find EOCD\"))",
  "kind": "failed"
}
```

**Interpretation:** `DocxFile::from_file` fails at ZIP open — immediate `ConvertError::Failed`. No alternate parser, no partial output. Classic **fail-fast**.

---

## Case 6: Partial success PDF (best-effort bonus)

- **File:** `tests/fixtures/pdf/needs_ocr_untrusted_fallback.pdf`
- **Intent:** page flagged `needs_ocr`, OCR disabled → keep untrusted native text + warn
- **Command:** `./target/release/fileconv one-detailed tests/fixtures/pdf/needs_ocr_untrusted_fallback.pdf --no-pdf-ocr`
- **Exit code:** 0
- **Outcome:** `partial_success` with `pdf_untrusted_text_fallback` warning

```json
{
  "format": "pdf",
  "markdown": "!!!@@@###$$$%%%^^^&&&***___+++===~~~[[[]]]{{{}}};;;:::???,,,...’\"\"ab\n\n",
  "outcome": "partial_success",
  "title": "needs_ocr_untrusted_fallback",
  "warnings": [
    {
      "code": "pdf_untrusted_text_fallback",
      "message": "trang 1: OCR thất bại — giữ text-layer/native không đáng tin (partial success)",
      "page": 1,
      "source": "pdf::needs_ocr_untrusted_pdfium"
    }
  ]
}
```

**Note:** With default OCR **on**, same file returned `full_success` (OCR produced output, no warning). Use `--no-pdf-ocr` to demonstrate best-effort untrusted-text path.

**Interpretation:** **Best-effort** — converter returns markdown + structured warning instead of failing silently. User/CLI sees `partial_success`, not a hard error.

---

## PDF fallback chain (AC)

Order in [`crates/core/src/conv/pdf/mod.rs`](crates/core/src/conv/pdf/mod.rs):

1. **pdf-inspector** — structured markdown per page; sets `needs_ocr` for scan/untrusted text-layer pages.
2. If inspector abandons or empty → **PDFium** ([`fallback.rs`](crates/core/src/conv/pdf/fallback.rs)) — char-count + OCR per page; [`recovery.rs`](crates/core/src/conv/pdf/recovery.rs) may keep untrusted native text with `ConversionWarning`.
3. If still empty / page filter fails → **pdf-extract** — last resort; `catch_unwind` maps panic to `ConvertErrorKind::Internal`.

**Case mapping:**

| Case | Which stage decided outcome |
|------|----------------------------|
| OK PDF (soak) | Inspector/PDFium succeeds early |
| Corrupt PDF | All stages fail → `failed` |
| Mismatch fake.pdf | Fails at PDF open/parse → `failed` |
| Partial PDF | PDFium recovery → untrusted text + warning |

---

## Best-effort vs fail-fast (AC)

| Style | Behavior | Example from this run |
|-------|----------|------------------------|
| **Best-effort / partial** | Output markdown + `warnings`, `outcome: partial_success` | Case 6 (`--no-pdf-ocr`) |
| **Fail-fast** | `exit 1`, JSON `{ kind, message }`, no markdown | Cases 3, 4, 5 |
| **Fallback chain** | Try next parser before hard fail | PDF Cases 2–4 (success only when a stage accepts input) |

**DOCX vs PDF:** DOCX has single parser (`docx-rust` + ZIP) — corrupt → Case 5 fail-fast. PDF tries multiple backends before giving up.

---

## Error types (issue template correction)

Legacy [`ConvertError`](crates/core/src/lib.rs): `BadPath` | `Unsupported` | `Failed(String)`.

Additive [`ConvertErrorKind`](crates/core/src/diagnostics.rs): `bad_path`, `unsupported`, `failed`, `dependency_missing`, `internal`.

CLI `one-detailed` surfaces `kind` + `message` on failure; success adds `outcome` + `warnings`.

Issue #361 lists `FormatNotSupported`, `IoError`, etc. — **those names are not in the current enum**; use the variants above when writing up results.

---

## Commands reference

```bash
cargo build --release -p fileconv-cli

# Fixtures prep
mkdir -p /tmp/intern361
cp tests/fixtures/sample/document.txt /tmp/intern361/ok.txt
cp bench/markhand_web/soak/fixtures/soak-pdf.pdf /tmp/intern361/ok.pdf
cp tests/fixtures/pdf/needs_ocr_untrusted_fallback.pdf /tmp/intern361/corrupt-trunc.pdf
truncate -s -64 /tmp/intern361/corrupt-trunc.pdf
cp tests/fixtures/sample/document.txt /tmp/intern361/fake.pdf
printf 'PK\x03\x04not-a-valid-ooxml' > /tmp/intern361/malformed.docx

# Run cases
./target/release/fileconv one-detailed /tmp/intern361/ok.txt
./target/release/fileconv one-detailed /tmp/intern361/ok.pdf
./target/release/fileconv one-detailed /tmp/intern361/corrupt-trunc.pdf
./target/release/fileconv one-detailed /tmp/intern361/fake.pdf
./target/release/fileconv one-detailed /tmp/intern361/malformed.docx
./target/release/fileconv one-detailed tests/fixtures/pdf/needs_ocr_untrusted_fallback.pdf --no-pdf-ocr
```
