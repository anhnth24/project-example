# Báo cáo Backend `fileconv-core` — Tốc độ & Độ chính xác

> Backend chuyển đổi file → Markdown, **viết bằng Rust và do mình làm chủ hoàn toàn**.
> `markitdown-rs` chỉ dùng **tham khảo** (đặt ở `vendor/markitdown-rs/`), không phải
> dependency. Kiểm thử trên **dữ liệu thật** (60 file) + corpus **ground-truth tiếng Việt**.
> Ngày chạy: 2026-06-30. Máy: Intel Xeon @ 2.80GHz, 4 nhân, build `--release`.

## 1. Kiến trúc — code của mình

`crates/core` tự cài đặt từng converter, **gọi thẳng** các crate Rust gốc (không qua
markitdown-rs):

| Định dạng | Crate gốc | Ghi chú cài đặt |
|---|---|---|
| pdf | `pdf-extract` (pin `=0.8.2`) | bọc `catch_unwind` để không sập vì PDF lỗi |
| docx | `docx-rust` | **phát hiện heading** qua style + bảng Markdown |
| pptx | `zip` + `quick-xml` | đọc slide **đúng thứ tự số**, gồm text trong bảng |
| xlsx | `calamine` | đọc **TẤT CẢ sheet** (+ hỗ trợ `.xls`) |
| csv | `csv` | xuất **bảng Markdown** đúng chuẩn |
| html | `htmd` (html5ever) | thay `html2md` — hết lỗi phình output |
| ảnh (OCR) | `tesseract` CLI (`vie+eng`) | OCR tiếng Việt |
| audio | `whisper-rs` + `symphonia` | phiên âm tiếng Việt, decode thuần Rust + resample 16kHz |

Toàn bộ là binary Rust native; **đã bỏ** dependency LLM nặng (`rig-core`/`tokio`) và
mọi `println!` debug mà markitdown-rs có.

## 2. Các lỗi đã phát hiện & ĐÃ SỬA trong bản viết lại

| # | Lỗi ở markitdown-rs | Cách sửa | Kết quả đo |
|---|---|---|---|
| 1 | `html2md` **phình output** (88 triệu ký tự từ file 2.2MB) | dùng `htmd` | `wiki-vietnam-vi`: **88.5M → 0.99M ký tự**, **1036ms → 83ms** |
| 2 | `xlsx` chỉ đọc **sheet đầu**, `.xls` lỗi | `calamine` lặp mọi sheet + `open_workbook_auto` | đọc đủ sheet, `.xls` OK |
| 3 | `docx` **mất cấu trúc** (chỉ nối text) | đọc style → `#` heading, bảng Markdown | giữ heading/bảng |
| 4 | `pptx` sai **thứ tự slide** + `println!` debug | sort theo số slide, parser sạch | đúng thứ tự, không rác |
| 5 | `pdf-extract` 0.12 **panic** làm sập cả tiến trình | `catch_unwind` + pin `0.8.2` | 60/60 file không sập |
| 6 | kéo **dep LLM nặng** + nhiều `println!` | bỏ hẳn | build gọn, output sạch |

## 3. Kết quả TỐC ĐỘ (release, 10 file/loại, 60 file)

| Loại | Số file | OK | ms/file (TB) | ms/page (TB) | KB/s (TB) |
|---|--:|--:|--:|--:|--:|
| pptx | 10 | 10 | **0.23** | **0.14** | 239 761 |
| xlsx | 10 | 10 | **0.23** | — | 153 678 |
| csv  | 10 | 10 | **1.07** | — | 15 531 |
| docx | 10 | 10 | **0.83** | — | 22 476 |
| html | 10 | 10 | **17.33** | — | 22 158 |
| pdf  | 10 | 10 | **244.9** | **17.6** | 8 387 |

**100% (60/60) file convert thành công.** So với bản bọc markitdown-rs: **html nhanh
hơn ~7×** (17ms vs 132ms) và hết phình; pdf tương đương (~18 ms/trang, vẫn là khâu
nặng nhất do `pdf-extract` dựng layout).

## 4. Kết quả ĐỘ CHÍNH XÁC NỘI DUNG (tiếng Việt)

Đo CER/WER (Levenshtein) sau khi **chuẩn hoá bỏ ký hiệu cấu trúc Markdown** (dấu `|`,
dòng `---`, `#`, `-`) — để bảng/heading mà backend thêm (đúng chức năng) không bị tính
là "sai chữ". Độ chính xác = (1 − CER)×100.

| Kịch bản | Độ chính xác | Ghi chú |
|---|--:|---|
| docx text | **100.0%** | hoàn hảo |
| csv text | **100.0%** | hoàn hảo (đã sửa: trước bị markup làm giảm) |
| xlsx text | **98.5%** | dư nhãn "## Sheet"; chữ Việt đúng 100% |
| pptx text | **98.0%** | dư nhãn "## Slide N"; chữ Việt đúng 100% |
| html text | **99.2%** | gần hoàn hảo |
| **ảnh chữ in (OCR vie)** | **98.5%** | Tesseract tiếng Việt rất tốt |
| ảnh scan kém (OCR) | **81.0%** | phân giải thấp |
| ảnh "viết tay" (OCR) | **47.9%** | *mô phỏng bằng font, không phải chữ viết tay thật* |

**Kết luận**: chữ Việt (dấu thanh, ơ/ư/ă/đ) được giữ gần như tuyệt đối ở mọi định dạng
văn bản và ở OCR ảnh chữ in. Điểm trừ nhỏ của pptx/xlsx là **nhãn cấu trúc** chứ không
phải lỗi nội dung.

## 4b. Kết quả AUDIO (whisper, tiếng Việt)

Dữ liệu: 4 clip tiếng Việt sinh bằng gTTS (giọng TTS tổng hợp, có ground-truth chính xác).
RTF = thời-gian-suy-luận / độ-dài-audio (<1 = nhanh hơn thời gian thực). Máy CPU 4 nhân.

| Model | Cỡ | Độ chính xác TB | WER TB | RTF TB | Nhận xét |
|---|--:|--:|--:|--:|---|
| ggml-tiny | 77 MB | **86.8%** | 0.256 | **0.15** | nhanh nhất, chính xác thấp |
| ggml-base | 148 MB | **94.5%** | 0.122 | **0.30** | cân bằng tốt (khuyến nghị) |
| ggml-small | 488 MB | **97.0%** | 0.052 | **0.99** | chính xác nhất, ~thời gian thực |

- Càng model lớn càng chính xác nhưng chậm hơn. **base** là điểm cân bằng (94.5%, nhanh ~3× thời gian thực).
- Decode + resample (symphonia, Rust thuần) chỉ ~5 ms/clip — không đáng kể so với suy luận.
- **Cache model**: `Converter` load model 1 lần rồi giữ lại (OnceLock) — load mất
  tiny 112 ms / base 167 ms / small 433 ms; các file sau **không phải load lại**,
  chỉ tốn thời gian suy luận. Convert 10 file base → tiết kiệm ~1.5 s.
- **Tăng tốc GPU/BLAS**: build với feature tương ứng (máy này chỉ có CPU nên chưa đo):
  `cargo build --release --features cuda` (NVIDIA), `--features metal` (macOS),
  `--features vulkan`, hoặc `--features openblas` (CPU). GPU thường nhanh hơn nhiều lần.
- *Lưu ý*: đây là giọng **TTS tổng hợp** (rõ ràng). Giọng người thật/nhiễu nền sẽ khó hơn;
  cần mẫu có nhãn (vd Common Voice tiếng Việt) để đo sát thực tế.

## 5. Hạn chế còn lại & hướng tiếp

- **OCR chữ viết tay**: Tesseract không hợp (47.9% trên mẫu mô phỏng). Cần engine khác
  (PaddleOCR / TrOCR / vision-LLM) nếu yêu cầu chữ viết tay tiếng Việt thật. *Lưu ý:
  mẫu hiện tại render bằng font, chưa phải chữ viết tay người — cần ảnh có nhãn thật để
  đo chính xác.*
- **PDF**: `pdf-extract` panic trên PDF phức tạp (đã chặn bằng catch_unwind + pin 0.8.2);
  cân nhắc `pdfium-render` nếu cần độ bền/tốc độ cao hơn. PDF scan ảnh cần OCR (chưa làm).
- **Audio**: đã có (whisper-rs). Còn có thể: cache model giữa các file, chạy GPU (CUDA),
  và kiểm thử trên giọng người thật có nhãn (Common Voice vi) thay vì chỉ TTS.

## 6. Tái lập

```bash
bash bench/download_corpus.sh                 # 60 file thật
python3 bench/make_vn_corpus.py && bash bench/make_vn_images.sh   # corpus tiếng Việt + ảnh
python3 bench/make_vn_audio.py                # audio tiếng Việt (gTTS)
bash bench/download_models.sh                 # model whisper tiny/base/small
cargo build --release
./target/release/fileconv speed    bench/corpus                 bench/REPORT_SPEED.md
./target/release/fileconv accuracy bench/vn_corpus/manifest.tsv  bench/REPORT_ACCURACY.md
./target/release/fileconv audio    models/ggml-tiny.bin,models/ggml-base.bin,models/ggml-small.bin \
                                   bench/vn_audio/manifest.tsv   bench/REPORT_AUDIO.md
```

Chi tiết: `bench/REPORT_SPEED.md`, `bench/REPORT_ACCURACY.md`, `bench/REPORT_AUDIO.md`.
Source tham khảo (không build): `vendor/markitdown-rs/`.
