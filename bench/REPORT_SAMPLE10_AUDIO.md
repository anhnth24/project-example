# Báo cáo AUDIO (whisper, tiếng Việt) — fileconv-core

RTF = thời gian suy luận / độ dài audio (càng nhỏ càng nhanh; <1 = nhanh hơn thời gian thực). Độ chính xác = (1 − CER)×100.

| Model | Clip | Kịch bản | Audio (s) | Decode (ms) | Infer (ms) | RTF | CER | WER | Độ chính xác |
|---|---|---|--:|--:|--:|--:|--:|--:|--:|
| ggml-base | en_test.mp3 | audio | 4.20 | 9 | 5033 | 1.20 | 0.000 | 0.000 | **100.0%** |
| ggml-base | vn_baocao.mp3 | audio | 5.14 | 5 | 2035 | 0.40 | 0.115 | 0.267 | **88.5%** |
| ggml-base | vn_camon.mp3 | audio | 4.70 | 3 | 1610 | 0.34 | 0.036 | 0.071 | **96.4%** |
| ggml-base | vn_giaoban.mp3 | audio | 4.97 | 3 | 1569 | 0.32 | 0.266 | 0.400 | **73.4%** |
| ggml-base | vn_sodienthoai.mp3 | audio | 6.19 | 3 | 1469 | 0.24 | 0.487 | 0.611 | **51.3%** |

## Trung bình theo model

Model được **load 1 lần rồi cache** (cột *Load model*); convert các file sau chỉ tốn thời gian suy luận, không load lại.

| Model | Load model 1 lần (ms) | Số clip | Độ chính xác TB | WER TB | RTF TB |
|---|--:|--:|--:|--:|--:|
| ggml-base | 13979 | 5 | **81.9%** | 0.270 | 0.50 |

