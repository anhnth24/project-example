# Báo cáo ĐỘ CHÍNH XÁC (tiếng Việt) — fileconv-core

Độ chính xác ký tự = (1 − CER)×100. CER/WER tính bằng khoảng cách Levenshtein trên text đã chuẩn hoá khoảng trắng.

| File | Kịch bản | Ref ký tự | Hyp ký tự | CER | WER | Độ chính xác % | ms |
|---|---|--:|--:|--:|--:|--:|--:|
| vn.docx | docx-text | 399 | 399 | 0.000 | 0.000 | **100.0%** | 9.8 |
| vn.pptx | pptx-text | 399 | 407 | 0.020 | 0.022 | **98.0%** | 0.3 |
| vn.xlsx | xlsx-text | 399 | 405 | 0.015 | 0.011 | **98.5%** | 0.3 |
| vn.csv | csv-text | 399 | 399 | 0.000 | 0.000 | **100.0%** | 0.1 |
| vn.html | html-text | 399 | 402 | 0.008 | 0.011 | **99.2%** | 0.1 |
| vn_printed.png | image-print-OCR | 399 | 399 | 0.015 | 0.067 | **98.5%** | 567.7 |
| vn_lowres.png | image-lowres-OCR | 399 | 333 | 0.190 | 0.270 | **81.0%** | 461.0 |
| vn_hand_caveat.png | handwrite-OCR | 399 | 337 | 0.376 | 0.888 | **62.4%** | 737.5 |
| vn_hand_dancing.png | handwrite-OCR | 399 | 172 | 0.667 | 0.888 | **33.3%** | 412.5 |

## Trung bình theo kịch bản

| Kịch bản | Số mẫu | Độ chính xác TB % | CER TB | WER TB |
|---|--:|--:|--:|--:|
| csv-text | 1 | **100.0%** | 0.000 | 0.000 |
| docx-text | 1 | **100.0%** | 0.000 | 0.000 |
| handwrite-OCR | 2 | **47.9%** | 0.521 | 0.888 |
| html-text | 1 | **99.2%** | 0.008 | 0.011 |
| image-lowres-OCR | 1 | **81.0%** | 0.190 | 0.270 |
| image-print-OCR | 1 | **98.5%** | 0.015 | 0.067 |
| pptx-text | 1 | **98.0%** | 0.020 | 0.022 |
| xlsx-text | 1 | **98.5%** | 0.015 | 0.011 |

