# Báo cáo ĐỘ CHÍNH XÁC (tiếng Việt) — fileconv-core

Độ chính xác ký tự = (1 − CER)×100. CER/WER tính bằng khoảng cách Levenshtein trên text đã chuẩn hoá khoảng trắng.

| File | Kịch bản | Ref ký tự | Hyp ký tự | CER | WER | Độ chính xác % | ms |
|---|---|--:|--:|--:|--:|--:|--:|
| vn.docx | docx-text | 399 | 399 | 0.000 | 0.000 | **100.0%** | 10.9 |
| vn.pptx | pptx-text | 399 | 419 | 0.075 | 0.169 | **92.5%** | 0.2 |
| vn.xlsx | xlsx-text | 399 | 431 | 0.080 | 0.169 | **92.0%** | 0.2 |
| vn.csv | csv-text | 399 | 399 | 0.000 | 0.000 | **100.0%** | 0.0 |
| vn.html | html-text | 399 | 402 | 0.008 | 0.011 | **99.2%** | 1.0 |
| vn_printed.png | image-print-OCR | 399 | 399 | 0.015 | 0.067 | **98.5%** | 706.9 |
| vn_lowres.png | image-lowres-OCR | 399 | 333 | 0.190 | 0.270 | **81.0%** | 601.3 |
| vn_hand_caveat.png | handwrite-OCR | 399 | 337 | 0.376 | 0.888 | **62.4%** | 938.0 |
| vn_hand_dancing.png | handwrite-OCR | 399 | 172 | 0.667 | 0.888 | **33.3%** | 505.0 |

## Trung bình theo kịch bản

| Kịch bản | Số mẫu | Độ chính xác TB % | CER TB | WER TB |
|---|--:|--:|--:|--:|
| csv-text | 1 | **100.0%** | 0.000 | 0.000 |
| docx-text | 1 | **100.0%** | 0.000 | 0.000 |
| handwrite-OCR | 2 | **47.9%** | 0.521 | 0.888 |
| html-text | 1 | **99.2%** | 0.008 | 0.011 |
| image-lowres-OCR | 1 | **81.0%** | 0.190 | 0.270 |
| image-print-OCR | 1 | **98.5%** | 0.015 | 0.067 |
| pptx-text | 1 | **92.5%** | 0.075 | 0.169 |
| xlsx-text | 1 | **92.0%** | 0.080 | 0.169 |

