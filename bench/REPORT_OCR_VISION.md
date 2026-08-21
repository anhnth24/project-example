# Báo cáo OCR vision-LLM — baseline CER (ADR 0016)

Ngày đo: 2026-08-21. Harness: `fileconv accuracy` trên 5 mẫu OCR (4 ảnh
[`bench/vn_corpus/`](vn_corpus/) + PDF scan [`gold-004.pdf`](../markhand_web/golden/documents/gold-004.pdf)).
Engine: OpenRouter vision (mặc định `qwen/qwen3.7-flash`, ADR 0016). Issue: [#416](https://github.com/anhnth24/project-example/issues/416).

Chi tiết terminal evidence, manifest tái lập, so sánh Tesseract historical và
acceptance checklist → comment issue #416 / PR description (format inter-v2-07).

---

# Báo cáo ĐỘ CHÍNH XÁC (tiếng Việt) — fileconv-core

Độ chính xác ký tự = (1 − CER)×100. CER/WER tính bằng khoảng cách Levenshtein trên text đã chuẩn hoá khoảng trắng.

| File | Kịch bản | Ref ký tự | Hyp ký tự | CER | WER | Độ chính xác % | ms |
|---|---|--:|--:|--:|--:|--:|--:|
| vn_printed.png | image-print-OCR | 399 | 399 | 0.013 | 0.034 | **98.7%** | 5453.5 |
| vn_lowres.png | image-lowres-OCR | 399 | 391 | 0.053 | 0.135 | **94.7%** | 5300.1 |
| vn_hand_caveat.png | handwrite-OCR | 399 | 434 | 0.150 | 0.876 | **85.0%** | 6371.2 |
| vn_hand_dancing.png | handwrite-OCR | 399 | 399 | 0.005 | 0.022 | **99.5%** | 5132.5 |
| gold-004.pdf | pdf-scan-OCR | 208 | 231 | 0.111 | 0.106 | **88.9%** | 4579.1 |

## Trung bình theo kịch bản

| Kịch bản | Số mẫu | Độ chính xác TB % | CER TB | WER TB |
|---|--:|--:|--:|--:|
| handwrite-OCR | 2 | **92.2%** | 0.078 | 0.449 |
| image-lowres-OCR | 1 | **94.7%** | 0.053 | 0.135 |
| image-print-OCR | 1 | **98.7%** | 0.013 | 0.034 |
| pdf-scan-OCR | 1 | **88.9%** | 0.111 | 0.106 |

## So sánh vision vs Tesseract (historical)

Nguồn: [`bench/REPORT.md`](REPORT.md) §4 (2026-06-30). Tesseract removed 2026-08-10.

| Kịch bản | Tesseract (historical) | Vision (2026-08-21) |
|----------|------------------------|---------------------|
| image-print-OCR | 99.2% | 98.7% |
| image-lowres-OCR | 99.0% | 94.7% |
| handwrite-OCR (TB) | ~47.9% | 92.2% |
| pdf-scan-OCR | ~1200 ms/trang (latency) | 88.9% CER / ~4580 ms |

## Observations (tiếng Việt)

- Dấu thanh và số (`1.234.567`) ổn trên ảnh chữ in; lỗi lẻ hoa-thường và ký tự hiếm.
- Outlier: `vn_lowres` (94.7%) — Tesseract cũ có tiền xử lý upscale local (~99%).
- Font viết tay mô phỏng: vision 85–99.5% vs Tesseract historical ~40%.
- `gold-004`: CER 88.9% một phần do GT Markdown vs OCR plain text + marker trang.
- Latency ~5.4 s/file (API network); trade-off cost/latency vs OCR local CPU.
