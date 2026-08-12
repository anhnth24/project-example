# Issue #363 — PDF pipeline optimization (intern-25)

> Corpus: `bench/sample10/pdf/` (5 arXiv PDF). Build: `cargo build --release -p fileconv-cli`.
> Machine: 16 CPU, WSL2. Ngày: 11/08/2026.

## Bottleneck

**Category:** parsing (pdf-inspector duplicate work)

Trên slow path (`<16` trang, không hit parallel fast path), pipeline gọi
`pdf_inspector::extract_pages_markdown_mem` **hai lần**:

1. `probe_pages_needing_ocr()` — trước fast path, luôn chạy
2. `via_pdf_inspector()` — spawn thread gọi lại cùng API

Fast path thành công vẫn trả tiền probe dù không cần `needs_ocr` cho fallback.

Evidence: baseline `fileconv speed bench/sample10/pdf` — **768 ms/file TB**;
sau khi bỏ duplicate — **160 ms/file TB** (xem bảng dưới).

## Optimization

**Một thay đổi:** defer probe + reuse single extract trên slow path.

- Bỏ `probe_pages_needing_ocr()` upfront trong [`mod.rs`](../crates/core/src/conv/pdf/mod.rs)
- Fast path (`filtered` / `parallel`) return sớm — **không extract/probe**
- Slow path: `extract_pages_markdown_mem()` một lần → truyền vào `via_pdf_inspector(prefetched)`
- Helper dùng chung: `extract_pages_markdown_mem`, `pages_needing_ocr_from_extract`

Không đổi algorithm pdf-inspector, PDFium, OCR.

## Before / after

| Metric | Before | After | Δ |
|--------|-------:|------:|--:|
| ms/file TB (5 PDF) | 768.47 | 159.69 | **−79.2%** |
| arxiv-1301.3781.pdf | 357.73 ms | 59.25 ms | **−83.4%** |
| arxiv-1409.1556.pdf | 597.69 ms | 126.76 ms | −78.8% |
| arxiv-1512.03385.pdf | 598.22 ms | 142.47 ms | −76.2% |
| arxiv-1706.03762.pdf | 1259.74 ms | 277.87 ms | −77.9% |
| arxiv-1810.04805.pdf | 1028.95 ms | 192.09 ms | −81.3% |
| ms/page TB | 55.69 | 11.57 | −79.2% |

## Regression

Markdown output **unchanged** — `diff` 5/5 file `bench/sample10/pdf/*.pdf` identical
(snapshots lưu tại `/tmp/issue363-regression/`).

`cargo test -p fileconv-core pdf::` — 28 passed.

## Issue

Closes https://github.com/anhnth24/project-example/issues/363
