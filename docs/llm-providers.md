# LLM providers cho Markhand Intelligence

Markhand chạy convert, OCR, quality, hybrid search và BRD/PRD deterministic mà
không cần LLM. LLM chỉ tổng hợp các citation đã retrieval khi người dùng bật rõ.

## Bốn trạng thái Q&A

| Trạng thái | Retrieval | Trả lời | Dữ liệu ra ngoài |
|---|---|---|---|
| Không cấu hình LLM | SQLite FTS5 + vector local | Extractive + citation | Không |
| LLM local đang chạy | Hybrid local | LLM tổng hợp + citation | Không |
| LLM cloud | Hybrid local | Cloud tổng hợp top-K | Chỉ top-K citation |
| Provider lỗi / thiếu key | Hybrid local | Tự fallback extractive | Không |

Provider không bao giờ là dependency bắt buộc của index/search. Nếu endpoint mất
kết nối, model chưa load hoặc key hết hạn, câu hỏi vẫn trả kết quả local kèm cảnh
báo thay vì làm hỏng toàn bộ tác vụ. Endpoint không kết nối được có connect
timeout 5 giây; model đang sinh câu trả lời có timeout tổng 120 giây.

## Khuyến nghị: self-host local

### Ollama

```bash
ollama serve
ollama pull qwen2.5:7b
```

Preset:

```text
Provider: Ollama
Base URL: http://127.0.0.1:11434
Model: qwen2.5:7b
API key: để trống
```

### LM Studio

Mở Local Server trong LM Studio:

```text
Base URL: http://127.0.0.1:1234
Model: tên model server đang expose
```

### llama.cpp server

```text
Base URL: http://127.0.0.1:8080
Model: local-model
```

### vLLM

```text
Base URL: http://127.0.0.1:8000
Model: tên model đã serve
```

Các local provider dùng OpenAI-compatible `/v1/chat/completions` và không bắt
buộc API key.

## Cloud presets

- OpenAI
- Anthropic Claude
- Google Gemini
- OpenRouter
- Groq
- Mistral AI
- Together AI
- Custom OpenAI-compatible

Cloud preset yêu cầu API key nếu provider bắt buộc. API key nhập trong desktop
chỉ giữ trong memory, không ghi vào `settings.json`. Muốn cấu hình ổn định qua
lần khởi động:

```bash
export FILECONV_LLM_PROVIDER=ollama
export FILECONV_LLM_BASE_URL=http://127.0.0.1:11434
export FILECONV_LLM_MODEL=qwen2.5:7b
# Cloud only:
export FILECONV_LLM_API_KEY=...
```

## Luồng Q&A

```text
DATA Markdown
→ heading chunks
→ SQLite FTS5 + vector hashing local (persist)
→ Reciprocal Rank Fusion + heading/token rerank
→ top citations
→ LLM provider (nếu bật)
→ kiểm tra citation
→ answer; hoặc fallback extractive nếu provider/grounding lỗi
```

Markhand không gửi toàn bộ DATA root. Handoff gửi tối đa 40 citation, mỗi citation
tối đa 600 ký tự. Q&A chỉ gửi các citation top-K của câu hỏi.

Index nằm tại `DATA/.markhand/knowledge.sqlite`, được cập nhật theo content hash
sau mỗi lần convert. Vector hashing 256 chiều chạy hoàn toàn local và là baseline
không phụ thuộc model embedding; đây là feature vector cho retrieval, không được
quảng cáo là neural semantic embedding.

## Quyền riêng tư

- Local preset: dữ liệu không rời máy.
- Cloud preset: UI hiển thị cảnh báo trước khi bật.
- PII scan chạy local.
- LLM artifacts luôn cần review; validation hiển thị áp dụng cho baseline
  deterministic.
- Vision OCR gửi toàn bộ ảnh tới provider đã cấu hình.

## Kiểm tra kết nối

Nút **Test kết nối** lưu cấu hình (không persist secret), gửi prompt `ping` và
hiển thị model, latency, endpoint local/cloud cùng response rút gọn.
