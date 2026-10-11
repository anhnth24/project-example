# Báo cáo ĐỘ CHÍNH XÁC (tiếng Việt) — fileconv-core

Độ chính xác ký tự = (1 − CER)×100. CER/WER tính bằng khoảng cách Levenshtein trên text đã chuẩn hoá khoảng trắng.

| File | Kịch bản | Ref ký tự | Hyp ký tự | CER | WER | Độ chính xác % | ms |
|---|---|--:|--:|--:|--:|--:|--:|
| 01_cong_van.pdf | pdf-cong-van-bo-tu-phap | 124 | 123 | 0.008 | 0.074 | **99.2%** | 11.8 |
| 02_quyet_dinh.pdf | pdf-quyet-dinh-thu-tuong | 132 | 121 | 0.083 | 0.138 | **91.7%** | 3.3 |
| 03_thong_tu.pdf | pdf-thong-tu-lien-tich | 152 | 145 | 0.086 | 0.114 | **91.4%** | 3.3 |
| 04_nghi_dinh.docx | docx-nghi-dinh-chinh-phu | 116 | 125 | 0.121 | 0.280 | **87.9%** | 5.8 |
| 05_giay_moi.docx | docx-giay-moi-ubnd | 133 | 139 | 0.075 | 0.200 | **92.5%** | 5.7 |

## Trung bình theo kịch bản

| Kịch bản | Số mẫu | Độ chính xác TB % | CER TB | WER TB |
|---|--:|--:|--:|--:|
| docx-giay-moi-ubnd | 1 | **92.5%** | 0.075 | 0.200 |
| docx-nghi-dinh-chinh-phu | 1 | **87.9%** | 0.121 | 0.280 |
| pdf-cong-van-bo-tu-phap | 1 | **99.2%** | 0.008 | 0.074 |
| pdf-quyet-dinh-thu-tuong | 1 | **91.7%** | 0.083 | 0.138 |
| pdf-thong-tu-lien-tich | 1 | **91.4%** | 0.086 | 0.114 |

