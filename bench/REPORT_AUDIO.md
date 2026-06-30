# Báo cáo AUDIO (whisper, tiếng Việt) — fileconv-core

RTF = thời gian suy luận / độ dài audio (càng nhỏ càng nhanh; <1 = nhanh hơn thời gian thực). Độ chính xác = (1 − CER)×100.

| Model | Clip | Kịch bản | Audio (s) | Decode (ms) | Infer (ms) | RTF | CER | WER | Độ chính xác |
|---|---|---|--:|--:|--:|--:|--:|--:|--:|
| ggml-tiny | clip1.mp3 | tts-vi | 6.31 | 6 | 948 | 0.15 | 0.159 | 0.278 | **84.1%** |
| ggml-tiny | clip2.mp3 | tts-vi | 7.01 | 6 | 988 | 0.14 | 0.047 | 0.143 | **95.3%** |
| ggml-tiny | clip3.mp3 | tts-vi | 5.45 | 5 | 977 | 0.18 | 0.081 | 0.176 | **91.9%** |
| ggml-tiny | clip4.mp3 | tts-vi | 6.50 | 6 | 1075 | 0.17 | 0.241 | 0.429 | **75.9%** |
| ggml-base | clip1.mp3 | tts-vi | 6.31 | 5 | 1927 | 0.31 | 0.000 | 0.000 | **100.0%** |
| ggml-base | clip2.mp3 | tts-vi | 7.01 | 6 | 1920 | 0.27 | 0.059 | 0.190 | **94.1%** |
| ggml-base | clip3.mp3 | tts-vi | 5.45 | 5 | 1874 | 0.34 | 0.041 | 0.059 | **95.9%** |
| ggml-base | clip4.mp3 | tts-vi | 6.50 | 6 | 2119 | 0.33 | 0.120 | 0.238 | **88.0%** |
| ggml-small | clip1.mp3 | tts-vi | 6.31 | 5 | 7172 | 1.14 | 0.045 | 0.056 | **95.5%** |
| ggml-small | clip2.mp3 | tts-vi | 7.01 | 7 | 6451 | 0.92 | 0.012 | 0.048 | **98.8%** |
| ggml-small | clip3.mp3 | tts-vi | 5.45 | 5 | 6203 | 1.14 | 0.027 | 0.059 | **97.3%** |
| ggml-small | clip4.mp3 | tts-vi | 6.50 | 6 | 6433 | 0.99 | 0.036 | 0.048 | **96.4%** |

## Trung bình theo model

Model được **load 1 lần rồi cache** (cột *Load model*); convert các file sau chỉ tốn thời gian suy luận, không load lại.

| Model | Load model 1 lần (ms) | Số clip | Độ chính xác TB | WER TB | RTF TB |
|---|--:|--:|--:|--:|--:|
| ggml-base | 167 | 4 | **94.5%** | 0.122 | 0.31 |
| ggml-small | 433 | 4 | **97.0%** | 0.052 | 1.05 |
| ggml-tiny | 112 | 4 | **86.8%** | 0.256 | 0.16 |

