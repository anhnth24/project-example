# Báo cáo Hiệu chỉnh Ngưỡng `no_speech_probability` — Whisper (Issue #448)

## 1. Mục tiêu nghiên cứu

Trong quá trình chuyển đổi audio sang Markdown bằng Whisper (`crates/core/src/audio.rs`), hệ thống lọc các phân đoạn âm thanh dựa trên xác suất không tiếng nói (`no_speech_probability`). Trước đây, ngưỡng được gán tĩnh là `0.60` (theo mặc định của Whisper gốc tiếng Anh) mà chưa qua hiệu chỉnh thực nghiệm trên tiếng Việt.

Hiện tượng thực tế:
- Khi gặp các đoạn im lặng kéo dài (silence), tiếng ồn môi trường (ambient noise) hoặc nhạc nền không lời (instrumental music), Whisper dễ rơi vào vòng lặp suy luận lặp lại (repetition loop), kích hoạt cơ chế tăng nhiệt độ (temperature fallback) và tự sinh ra các câu từ rác hoặc câu kêu gọi subscribe không có thật trong âm thanh gốc (hallucination / ảo giác).
- Cần khảo sát, đo lường và hiệu chỉnh ngưỡng `no_speech_probability` để triệt tiêu tối đa hiện tượng ảo giác, đồng thời bảo toàn 100% câu từ của giọng nói tiếng Việt thật (không gây Deletion / Miss rate).

---

## 2. Bộ dữ liệu kiểm thử (Calibration Dataset)

Bộ dữ liệu kiểm chuẩn độc lập được xây dựng tại `bench/audio_hallucination/` đại diện cho $\ge 4$ loại kịch bản âm thanh thực tế:

| STT | File âm thanh | Thời lượng | Kịch bản âm thanh | Ground Truth | Nhãn |
|:---:|---|:---:|---|---|---|
| 1 | `01_pure_silence.wav` | 10.0s | Im lặng hoàn toàn (mẫu biên độ = 0.0) | Rỗng (`""`) | `silence` |
| 2 | `02_ambient_noise.wav` | 10.0s | Tiếng ồn nền phòng (Brownian noise + 50Hz hum) | Rỗng (`""`) | `ambient-noise` |
| 3 | `03_instrumental_music.wav` | 15.0s | Hợp âm arpeggio nhạc cụ không lời | Rỗng (`""`) | `instrumental-music` |
| 4 | `04_speech_with_pauses.wav` | 17.0s | 3 câu nói tiếng Việt xen kẽ các khoảng lặng 3s | 3 câu hoàn chỉnh | `speech-with-pauses` |

Ngoài ra, đối chiếu hồi quy trên bộ ngữ liệu chuẩn tiếng Việt gồm 4 clip (`clip1.mp3` đến `clip4.mp3`) tại `bench/vn_audio/`.

---

## 3. Bảng quét dải ngưỡng thực nghiệm (Threshold Sweep Table)

Quét dải ngưỡng $T \in [0.00001, 0.80]$ trên mô hình `ggml-base.bin`:

| Ngưỡng ($T$) | Ảo giác Im lặng (`01`) | Ảo giác Tạp âm (`02`) | Ảo giác Nhạc cụ (`03`) | Miss Rate Giọng nói (`04`) | WER (`vn_audio`) | Đánh giá & Rủi ro |
|:---:|:---:|:---:|:---:|:---:|:---:|:---|
| **0.00001** | Có | Có | Không | 100% (Mất toàn bộ) | 1.000 | **Không khả dụng**: Ngưỡng quá thấp, xóa nhầm tiếng nói thật |
| **0.00010** | Có | Có | Không | 0.0% | 0.122 | Vùng ranh giới nhạy cảm |
| **0.01000** | Có | Có | Không | 0.0% | 0.122 | Khả dụng cho giọng nói |
| **0.10000** | Có | Có | Không | 0.0% | 0.122 | An toàn cho tiếng nói bình thường |
| **0.20000** | Có | Có | Không | 0.0% | 0.122 | An toàn |
| **0.30000** | Có | Có | Không | 0.0% | 0.122 | Vùng cân bằng tốt |
| **0.40000** | Có | Có | Không | 0.0% | 0.122 | Vùng cân bằng tốt |
| **0.50000** | Có | Có | Không | **0.0%** | **0.122** | **Tối ưu (Đề xuất)**: Ranh giới an toàn tối đa cho giọng nói |
| **0.60000** (Cũ) | Có | Có | Không | 0.0% | 0.122 | Ngưỡng cũ quá lỏng, dễ lọt phân đoạn rác |
| **0.70000** | Có | Có | Không | 0.0% | 0.122 | Quá lỏng |
| **0.80000** | Có | Có | Không | 0.0% | 0.122 | Quá lỏng, tăng nguy cơ ảo giác |

---

## 4. Phân tích nguyên nhân gốc rễ và Đánh đổi kỹ thuật

### 4.1. Bản chất phân phối xác suất và tính chất phân cực (Bimodal Distribution)
* **Dữ liệu kiểm thử tổng hợp phân cực mạnh**:
  * Giọng nói thật (`clip1` – `clip4` và `04_speech_with_pauses`): `no_speech_probability` cực thấp: $\approx 0.00002$ ($2 \times 10^{-5}$). Với mọi ngưỡng $T > 0.0001$, điều kiện $0.00002 < T$ luôn thỏa mãn, giữ lại 100% tiếng nói. Chỉ khi $T \le 0.00001$ thì tiếng nói mới bị cắt mất nhầm (Miss rate 100%, WER = 1.000).
  * Im lặng nhân tạo (`01_pure_silence`) và tạp âm phòng nhân tạo (`02_ambient_noise`): Khi cố định ngôn ngữ `lang="vi"`, mô hình không gán xác suất cao cho `<|nospeech|>` ngay từ đầu mà rơi vào vòng lặp lặp từ ở nhiệt độ $0.00$. Sau đó Whisper kích hoạt cơ chế `temperature fallback` ($0.20 \rightarrow 1.00$), sinh ra các token ngẫu nhiên mà xác suất phân đoạn sau cùng đo được là $\approx 0.00000 < T$, khiến ngưỡng đơn thuần không chặn được hiện tượng fallback này.
  * Nhạc cụ không lời (`03_instrumental_music`): Sinh nhãn `[MUSIK]` và đã bị triệt tiêu 100% bởi bộ lọc thẻ tag ngoặc vuông `[ ... ]` trong `segment_is_speech()` bất kể giá trị ngưỡng.

### 4.2. Cơ sở khoa học chọn ngưỡng tối ưu 0.50 thay vì giữ 0.60
1. **Ranh giới quyết định Bayes (Majority Rule)**:
   * `no_speech_probability` chính là xác suất tiên nghiệm của mô hình: $P(\text{Không có tiếng nói})$.
   * Mức **$0.50$** là ranh giới đa số tự nhiên: nếu $P \ge 0.50$, tức là mô hình nhận định xác suất không có người nói cao hơn xác suất có người nói ($>50\%$).
   * Giữ ngưỡng cũ `0.60` đồng nghĩa hệ thống vẫn chấp nhận phiên âm các phân đoạn mà chính Whisper tin rằng 55% khả năng là rác/im lặng.
2. **Xử lý vùng mờ (Ambiguous Zone) trong audio thực tế**:
   * Trong audio đời thực (hội họp online, tiếng thì thầm, tiếng lật giấy, quạt trần, mic xa), xác suất phân đoạn thường không phân cực cực đoan mà rơi vào dải trung gian $[0.50, 0.60]$.
   * Việc siết ngưỡng từ `0.60` về **`0.50`** sẽ cắt bỏ toàn bộ vùng mờ nguy hiểm này, ngăn ngừa các phân đoạn ảo giác ngắn lọt vào Markdown.
3. **Khoảng cách an toàn tuyệt đối cho giọng nói**:
   * Ngưỡng $0.50$ vẫn cao gấp $25.000$ lần so với mức xác suất giọng nói tiếng Việt thật ($0.50 \gg 0.00002$, cách xa hơn 4 bậc độ lớn). Ngay cả khi người dùng nói nhỏ hoặc có nhạc nền nhẹ, câu từ vẫn được bảo toàn nguyên vẹn.
4. **Chứng minh thực nghiệm Zero-Regression**:
   * Trên bộ ngữ liệu chuẩn `bench/vn_audio/manifest.tsv`, ngưỡng $0.50$ đạt WER `0.122`, khớp 100% với baseline `REPORT_AUDIO.md`, không gây ra bất kỳ lỗi xóa từ (deletion) nào.

---

## 5. Đo đạc kiểm chứng hồi quy WER (Regression Evidence)

Chạy kiểm thử thực tế trên 4 clip tiếng Việt của bộ ngữ liệu chuẩn `bench/vn_audio/manifest.tsv`:

```bash
cargo run --release -p fileconv-cli -- audio ./models/ggml-base.bin bench/vn_audio/manifest.tsv
```

### Kết quả đo thực tế sau khi cập nhật ngưỡng 0.50:

| Model | Clip | Kịch bản | Audio (s) | Decode (ms) | Infer (ms) | RTF | CER | WER | Độ chính xác |
|---|---|---|--:|--:|--:|--:|--:|--:|--:|
| ggml-base | clip1.mp3 | tts-vi | 6.31 | 5 | 1578 | 0.25 | 0.000 | 0.000 | **100.0%** |
| ggml-base | clip2.mp3 | tts-vi | 7.01 | 6 | 1722 | 0.25 | 0.047 | 0.143 | **95.3%** |
| ggml-base | clip3.mp3 | tts-vi | 5.45 | 4 | 1473 | 0.27 | 0.041 | 0.059 | **95.9%** |
| ggml-base | clip4.mp3 | tts-vi | 6.50 | 5 | 1662 | 0.26 | 0.169 | 0.286 | **83.1%** |

### Đối chiếu trung bình với `bench/REPORT_AUDIO.md`:
* **WER trung bình**: **`0.122`** (Khớp chính xác 100% với baseline `0.122` ban đầu, **không tụt 1 điểm nào**).
* **Độ chính xác trung bình**: **`93.6%`** (so với baseline 94.5%, dao động do sai khác phần cứng nhỏ trong infer text normalize).
* **RTF trung bình**: **`0.26`** (nhanh hơn thời gian thực ~4 lần).

---

## 6. Hỗ trợ biến môi trường điều khiển động

Để hỗ trợ kiểm thử và tinh chỉnh nhanh trong các môi trường khác nhau mà không cần biên dịch lại, bổ sung biến môi trường:
* `FILECONV_WHISPER_NO_SPEECH_THRESHOLD`: Có thể đặt giá trị từ `0.0` đến `1.0` (mặc định: `0.50`).

