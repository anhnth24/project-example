# Báo cáo AUDIO (whisper, tiếng Việt) — fileconv-core

RTF = thời gian suy luận / độ dài audio (càng nhỏ càng nhanh; <1 = nhanh hơn thời gian thực). Độ chính xác = (1 − CER)×100.

| Model | Clip | Kịch bản | Audio (s) | Decode (ms) | Infer (ms) | RTF | CER | WER | Độ chính xác |
|---|---|---|--:|--:|--:|--:|--:|--:|--:|
| ggml-base | vn_baocao.mp3 | audio | 5.14 | 4 | 3058 | 0.60 | 0.115 | 0.267 | **88.5%** |
| ggml-base | vn_camon.mp3 | audio | 4.70 | 5 | 1539 | 0.33 | 0.036 | 0.071 | **96.4%** |
| ggml-base | vn_giaoban.mp3 | audio | 4.97 | 3 | 2082 | 0.42 | 0.266 | 0.400 | **73.4%** |
| ggml-base | vn_sodienthoai.mp3 | audio | 6.19 | 4 | 2877 | 0.46 | 0.487 | 0.611 | **51.3%** |
| ggml-small | vn_baocao.mp3 | audio | 5.14 | 4 | 7844 | 1.53 | 0.049 | 0.067 | **95.1%** |
| ggml-small | vn_camon.mp3 | audio | 4.70 | 3 | 5081 | 1.08 | 0.036 | 0.071 | **96.4%** |
| ggml-small | vn_giaoban.mp3 | audio | 4.97 | 2 | 5142 | 1.03 | 0.234 | 0.400 | **76.6%** |
| ggml-small | vn_sodienthoai.mp3 | audio | 6.19 | 3 | 4992 | 0.81 | 0.590 | 0.611 | **41.0%** |
| ggml-PhoWhisper-small | vn_baocao.mp3 | audio | 5.14 | 3 | 5127 | 1.00 | 0.082 | 0.133 | **91.8%** |
| ggml-PhoWhisper-small | vn_camon.mp3 | audio | 4.70 | 2 | 6632 | 1.41 | 0.089 | 0.143 | **91.1%** |
| ggml-PhoWhisper-small | vn_giaoban.mp3 | audio | 4.97 | 3 | 5155 | 1.04 | 0.094 | 0.200 | **90.6%** |
| ggml-PhoWhisper-small | vn_sodienthoai.mp3 | audio | 6.19 | 3 | 5568 | 0.90 | 0.103 | 0.167 | **89.7%** |

## Trung bình theo model

Model được **load 1 lần rồi cache** (cột *Load model*); convert các file sau chỉ tốn thời gian suy luận, không load lại.

| Model | Load model 1 lần (ms) | Số clip | Độ chính xác TB | WER TB | RTF TB |
|---|--:|--:|--:|--:|--:|
| ggml-PhoWhisper-small | 1062 | 4 | **90.8%** | 0.161 | 1.09 |
| ggml-base | 12101 | 4 | **77.4%** | 0.337 | 0.45 |
| ggml-small | 36343 | 4 | **77.3%** | 0.287 | 1.11 |

