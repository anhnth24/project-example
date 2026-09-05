# Runbook: Cấu hình OpenRouter End-to-End & Bản đồ Biến Môi Trường (Env Map)

Tài liệu này cung cấp bản đồ biến môi trường (Environment Variable Map) hoàn chỉnh cho stack OpenRouter trong Markhand (gồm Vision OCR, Cloud Embedding, và Grounded Chat Q&A), giải thích chi tiết cơ chế phân chia prefix (`FILECONV_*` vs `MARKHAND_*`), phân biệt giữa profile Mock POC và profile OpenRouter thật, kèm checklist 10 bước chuẩn hóa cho thực tập sinh (intern).

---

## 1. Kiến trúc Tổng quan & Nguyên tắc Prefix

Hệ thống Markhand gồm nhiều tầng kiến trúc với ranh giới bảo mật nghiêm ngặt theo **ADR 0016**:
- **CLI / Desktop App (`fileconv`, Markhand Desktop)**: Chạy cục bộ trực tiếp trên máy người dùng, sử dụng các biến có tiền tố `FILECONV_*`.
- **Web Server API (`fileconv-server`)**: Cung cấp REST/SSE endpoints cho Web SPA (Auth, Document Management, Search, Grounded Chat Q&A), sử dụng các biến `MARKHAND_*`.
- **Background Workers (`fileconv-worker`)**: Các tiến trình xử lý ngầm (Convert, Index, Embedding, Delete, Reconcile), sử dụng các biến `MARKHAND_*`.
- **Sandbox Converter Isolation (Bảo mật tối quan trọng)**: Khi worker thực hiện convert file, nó gọi CLI `fileconv` bên trong Linux Landlock sandbox hoàn toàn **không có quyền truy cập mạng (no-network)**. Do đó, CLI trong sandbox không thể và **không bao giờ nhận API key**. Trang scan/ảnh được trích xuất thành ảnh JPEG (`--ocr-defer-dir`), sau đó worker đáng tin cậy bên ngoài sandbox mới đọc biến `MARKHAND_OCR_*` để gửi request tới OpenRouter.

### Quy tắc phân chia tiền tố (Prefix Rules):
1. **`FILECONV_OCR_*`**: Chỉ dành cho CLI độc lập và Markhand Desktop. Nếu không set `FILECONV_OCR_API_KEY`, hệ thống sẽ tự động fallback về `FILECONV_LLM_API_KEY`.
2. **`MARKHAND_OCR_*`**: Chỉ dành cho `worker-convert` của Markhand Web (chạy ở worker stage ngoài sandbox). **Không** dùng chung `FILECONV_OCR_*` cho worker.
3. **`MARKHAND_EMBEDDING_*`**: Dành cho `worker-embedding` và API service để sinh vector embedding và kiểm tra readiness probe.
4. **`MARKHAND_CHAT_*`**: Dành cho `fileconv-server` để thực hiện sinh câu trả lời trong Q&A pipeline.

---

## 2. Bản đồ Biến Môi Trường (OpenRouter Env Map)

Bảng dưới đây thống kê đầy đủ các biến môi trường liên quan trực tiếp hoặc gián tiếp đến OpenRouter trên toàn bộ hệ thống (gồm hơn 25 biến):

| Biến môi trường (`Env Var`) | Dùng ở đâu (`CLI / Worker / Web API`) | Bắt buộc khi nào (`Mandatory Condition`) | Ghi chú & Giá trị mặc định |
|---|---|---|---|
| **Nhóm 1: Vision OCR (CLI & Desktop)** | | | |
| `FILECONV_OCR_API_KEY` | CLI / Desktop | Bắt buộc khi convert ảnh / PDF scan trực tiếp từ CLI/Desktop bằng OpenRouter. | Tự động fallback về `FILECONV_LLM_API_KEY` nếu chưa đặt. |
| `FILECONV_LLM_API_KEY` | CLI / Desktop / MCP | Key dùng chung cho toàn bộ tính năng LLM (fallback cho OCR, MCP tools: `summarize`, `translate`, `ocr_hard`). | Khuyên dùng nếu muốn cấu hình 1 key duy nhất cho môi trường cá nhân CLI. |
| `FILECONV_OCR_BASE_URL` | CLI / Desktop | Tuỳ chọn; bắt buộc khi trỏ sang proxy riêng hoặc local vLLM/Ollama vision. | Mặc định: `https://openrouter.ai/api` |
| `FILECONV_OCR_MODEL` | CLI / Desktop | Tuỳ chọn; bắt buộc khi muốn đổi vision model khác. | Mặc định: `qwen/qwen3.7-flash` |
| `FILECONV_OCR_TIMEOUT_SECS` | CLI / Desktop | Tuỳ chọn; bắt buộc tăng khi mạng chậm hoặc ảnh độ phân giải cực cao. | Mặc định: `180` giây |
| `FILECONV_OCR_SYSTEM_PROMPT` | CLI / Desktop | Tuỳ chọn; bắt buộc khi cần format trích xuất chuyên biệt ngoài Markdown chuẩn. | Mặc định là prompt "chép trung thực" theo ADR 0016. |
| **Nhóm 2: Vision OCR (Server Workers)** | | | |
| `MARKHAND_OCR_API_KEY` | Worker (`worker-convert`) | Bắt buộc khi xử lý tài liệu scan/ảnh trên Web stack qua OpenRouter. | Thiếu key → job scan fail với lỗi `DependencyMissing: vision OCR not configured` (fail-closed). |
| `MARKHAND_OCR_BASE_URL` | Worker (`worker-convert`) | Tuỳ chọn; bắt buộc khi dùng self-hosted local vision endpoint. | Mặc định: `https://openrouter.ai/api` |
| `MARKHAND_OCR_MODEL` | Worker (`worker-convert`) | Tuỳ chọn; bắt buộc khi đổi vision model cho worker. | Mặc định: `qwen/qwen3.7-flash` |
| `MARKHAND_OCR_BATCH_PAGES` | Worker (`worker-convert`) | Tuỳ chọn; bắt buộc khi cần tinh chỉnh throughput/rate-limit. | Mặc định trong code là `5` trang/request (kẹp tối đa `16`). |
| `MARKHAND_OCR_TIMEOUT_SECS` | Worker (`worker-convert`) | Tuỳ chọn; cap timeout cho một batch request. | Mặc định: `300` giây (hoặc `60s + 15s × trang`). |
| `MARKHAND_OCR_SYSTEM_PROMPT` | Worker (`worker-convert`) | Tuỳ chọn; thay thế system prompt OCR cho server. | Mặc định: Giữ nguyên prompt "chép trung thực" chuẩn của server. |
| **Nhóm 3: Cloud Embeddings (API & Workers)** | | | |
| `MARKHAND_EMBEDDING_BASE_URL` | API & Worker (`worker-embedding`) | Bắt buộc khi chuyển sang dùng cloud embedding. | OpenRouter: `https://openrouter.ai/api/v1` |
| `MARKHAND_EMBEDDING_API_KEY` | API & Worker (`worker-embedding`) | Bắt buộc khi `MARKHAND_EMBEDDING_RUNTIME_PATH=provider-cloud`. | OpenRouter API Key (`sk-or-v1-...`). |
| `MARKHAND_EMBEDDING_PROVIDER` | API & Worker (`worker-embedding`) | Bắt buộc khi cấu hình embedding provider. | Giá trị: `openrouter` (hoặc `openai-compatible`). |
| `MARKHAND_EMBEDDING_MODEL` | API & Worker (`worker-embedding`) | Bắt buộc; chỉ định model embedding. | Đề xuất ADR 0016: `qwen/qwen3-embedding-8b`. |
| `MARKHAND_EMBEDDING_REVISION` | API & Worker (`worker-embedding`) | Bắt buộc; khóa snapshot model để đảm bảo tính bất biến của vector. | Giá trị pin: `qwen3-embedding-8b-20251028`. |
| `MARKHAND_EMBEDDING_DIMENSIONS` | API & Worker (`worker-embedding`) | Bắt buộc; số chiều vector lưu trữ vào Qdrant. | Khuyến nghị: `1024` (tận dụng Matryoshka Representation Learning - MRL). |
| `MARKHAND_EMBEDDING_RUNTIME_PATH` | API & Worker (`worker-embedding`) | Bắt buộc; phân biệt runtime on-prem vs cloud. | OpenRouter bắt buộc: `provider-cloud` (thay vì `local-neural`). |
| `MARKHAND_ALLOW_CLOUD_EMBEDDINGS` | API & Worker (`worker-embedding`) | **BẮT BUỘC**: Cờ opt-in cho phép egress dữ liệu chunk văn bản lên cloud (ADR 0016). | Bắt buộc: `true` (nếu thiếu, server fail-fast với `CloudRuntimeNotAllowed`). |
| `MARKHAND_EMBEDDING_NORMALIZE` | API & Worker (`worker-embedding`) | Bắt buộc khi dùng OpenRouter để chuẩn hóa L2 vector phía client. | Bắt buộc: `client` |
| `MARKHAND_EMBEDDING_SEND_DIMENSIONS` | API & Worker (`worker-embedding`) | Khuyến nghị khi dùng MRL dimensions. | Giá trị: `true` |
| `MARKHAND_INDEX_SIGNATURE` | API & Worker (`worker-embedding`) | **BẮT BUỘC**: Chữ ký SHA256 xác thực tính tương thích của collection Qdrant. | Sinh từ script `print-index-signature.py`. Sai chữ ký worker sẽ từ chối boot để chống rách/lệch index. |
| **Nhóm 4: Grounded Chat Q&A (API Server)** | | | |
| `MARKHAND_CHAT_BASE_URL` | Web API (`fileconv-server`) | Bắt buộc khi kích hoạt tính năng sinh câu trả lời bằng LLM (Q&A). | OpenRouter: `https://openrouter.ai/api/v1` (Fallback alias cũ `MARKHAND_GLM_BASE_URL` đã deprecated). |
| `MARKHAND_CHAT_API_KEY` | Web API (`fileconv-server`) | Bắt buộc khi kích hoạt LLM chat provider. | Khóa API OpenRouter (`sk-or-v1-...`). |
| `MARKHAND_CHAT_MODEL` | Web API (`fileconv-server`) | Bắt buộc khi dùng chat provider. | Khuyến nghị: `qwen/qwen3.7-flash` (nhanh, rẻ, hỗ trợ ngữ cảnh lớn). |
| `MARKHAND_CHAT_TIMEOUT_SECS` | Web API (`fileconv-server`) | Tuỳ chọn; timeout cho một lượt gọi chat LLM. | Mặc định: `30`s. Khuyên nâng lên `60-120`s khi dùng thinking model. |
| `MARKHAND_CHAT_REASONING` | Web API (`fileconv-server`) | Tuỳ chọn; điều khiển suy nghĩ của hybrid-thinking model (Qwen 3.x). | Đặt `off` để giảm độ trễ (tránh model suy nghĩ 20-60s trước khi trả lời). |
| `MARKHAND_CHAT_MAX_TOKENS` | Web API (`fileconv-server`) | Tuỳ chọn; giới hạn token câu trả lời. | Kẹp trong khoảng `64..=8192`. Giúp cắt đuôi khi model viết lan man. |
| `MARKHAND_QA_ALLOW_UNVERIFIED_LLM` | Web API (`fileconv-server`) | Tuỳ chọn (Dev/UAT only). | Đặt `1` để hiển thị mode `llm_unverified` kèm cảnh báo thay vì fail-closed extractive. |
| **Nhóm 5: Readiness & Hạ tầng** | | | |
| `MARKHAND_READY_PROBE_TIMEOUT_SECS` | Web API (`fileconv-server`) | Tuỳ chọn; thời gian chờ probe `/api/v1/health/ready`. | Mặc định `2`s; khuyên đặt `10`s khi dùng OpenRouter để tránh rớt TLS cold-start gây flapping 503. |
| `MARKHAND_CHAT_PIN_HOSTNAME` / `MARKHAND_CHAT_HOST_IPV4` | Web API (Compose container) | Tuỳ chọn khi Docker bridge không có IPv6 mà OpenRouter phân giải AAAA. | Dùng ghim IP v4 cho container. |

---

## 3. Phân biệt Profile Mock POC vs Profile OpenRouter Thật

Hệ thống hỗ trợ 2 chế độ vận hành chính nhằm tối ưu chi phí và tính bảo mật:

| Đặc điểm | Profile Mock POC (`COMPOSE_PROFILES=mock`) | Profile UAT / Production (OpenRouter thật) |
|---|---|---|
| **Mục đích** | Chạy CI/CD, unit/integration test, kiểm thử luồng nội bộ không tốn chi phí và không phụ thuộc mạng ngoài. | Đánh giá chất lượng thực tế (UAT), xử lý tài liệu scan thật, semantic search tiếng Việt chất lượng cao. |
| **Chi phí API & Token** | Hoàn toàn miễn phí ($0), không phát sinh request ra internet. | Tiêu tốn credit OpenRouter theo token (OCR ~$0.03/M, Embedding ~$0.01/M). |
| **Yêu cầu API Key** | Dùng key giả định (`poc-embedding-key`, `poc-chat-key`). | Bắt buộc phải có API key thật từ OpenRouter (`sk-or-v1-...`). |
| **Quyền Egress dữ liệu** | **Air-gapped / Local-only**: Dữ liệu văn bản và ảnh không rời khỏi máy chủ. | **Cloud Egress**: Ảnh scan và các đoạn văn bản (chunks) được gửi lên OpenRouter (yêu cầu opt-in tường minh). |
| **Hành vi OCR** | Ảnh scan trả về placeholder hoặc fail-closed nếu không cấu hình. | Worker gửi batch JPEG lên OpenRouter `qwen/qwen3.7-flash` và chép chính xác nội dung Markdown. |
| **Embedding Engine** | Container `mock-embedding` (Python script tạo vector giả lập 8 chiều). | `qwen/qwen3-embedding-8b` (1024 chiều MRL, client-side normalized). |
| **Index Signature** | `72dda20007ffb7fbe293612091103321eb9e4e0e4a0517a5f3413e31a2978874` | `229680cc2d8df20a0776d3c06b31f88a9d0f201f2047b3849e2c9ea47545629f` |
| **Chat Q&A** | Extractive fallback hoặc mock server trả về echo fixture. | Trả lời tự nhiên bằng tiếng Việt tổng hợp từ các đoạn trích dẫn (grounded citations). |

---

## 4. Hướng dẫn Chạy Thực Tế (Verification Evidence)

### 4.1. Xác nhận Mock Profile hoạt động độc lập (Không cần OpenRouter)
Chạy stack local với profile mock:
```bash
COMPOSE_PROFILES=mock deploy/scripts/up.sh
```
Kết quả kiểm tra sức khỏe thành công:
```
healthy: postgres
healthy: qdrant
healthy: minio
healthy: otel
healthy: mock-embedding
seeded local development metadata
```
Toàn bộ stack sẵn sàng mà không cần bất kỳ API key nào của bên thứ ba.

### 4.2. Kiểm chứng hành vi CLI OCR với các biến môi trường
1. **Trường hợp thiếu cấu hình (Fail-closed)**:
   ```bash
   ./target/release/fileconv one bench/markhand_web/golden/documents/gold-020.png
   ```
   *Kết quả*: Trả về lỗi `DependencyMissing` rõ ràng:
   ```
   Error: chuyển đổi thất bại: OCR vision: vision OCR chưa cấu hình; đặt FILECONV_OCR_API_KEY (OpenRouter) hoặc FILECONV_OCR_BASE_URL cho server vision local
   ```
2. **Trường hợp cấu hình qua `FILECONV_OCR_API_KEY` hoặc fallback `FILECONV_LLM_API_KEY`**:
   Khi đặt một trong hai biến trên, CLI tự động kích hoạt kết nối tới OpenRouter endpoint (`https://openrouter.ai/api`). Nếu key hợp lệ, văn bản sẽ được trích xuất thành Markdown chuẩn.

### 4.3. Kiểm chứng Web Worker Job với OpenRouter
Khi chuyển sang profile OpenRouter cho Web backend:
- `worker-convert` nhận file scan qua saga upload.
- Converter sandbox cô lập mạng render trang scan ra JPEG và trả cờ `ocr-pending`.
- `worker-convert` bên ngoài sandbox đọc `MARKHAND_OCR_API_KEY` gửi batch JPEG tới OpenRouter.
- `worker-embedding` đọc `MARKHAND_EMBEDDING_API_KEY` và `MARKHAND_ALLOW_CLOUD_EMBEDDINGS=true` để tạo vector 1024-d và lưu vào Qdrant với signature tương ứng.

---

## 5. Đề Xuất Cải Thiện Chú Thích trong `deploy/.env.example`

Trong file mẫu cấu hình `deploy/.env.example`, cần bổ sung các chú thích rõ ràng để tránh gây hiểu lầm cho người mới:
1. **Làm rõ ranh giới Prefix**:
   Giải thích rõ rằng các biến `MARKHAND_OCR_*` chỉ dành cho worker container, còn CLI độc lập và desktop dùng `FILECONV_OCR_*` (với fallback `FILECONV_LLM_API_KEY`). Converter sandbox không bao giờ nhận key.
2. **Cung cấp sẵn Index Signature của OpenRouter**:
   Thay vì chỉ để placeholder `<print-index-signature.py output>`, điền sẵn chữ ký đã tính toán chuẩn (`229680cc2d8df20a0776d3c06b31f88a9d0f201f2047b3849e2c9ea47545629f`) kèm lệnh sinh chữ ký để intern có thể đối chiếu ngay.
3. **Nhấn mạnh cờ bảo vệ Egress**:
   Ghi chú rõ ràng: Nếu thiếu `MARKHAND_ALLOW_CLOUD_EMBEDDINGS=true`, hệ thống sẽ từ chối khởi động embedding worker để bảo vệ dữ liệu.

---

## 6. Checklist 10 Bước Chuẩn Cho Thực Tập Sinh Mới (Reproducible Setup)

Thực hiện tuần tự 10 bước sau để thiết lập môi trường dev từ đầu đến khi tích hợp hoàn chỉnh OpenRouter:

- [ ] **Bước 1: Clone kho mã nguồn và kiểm tra công cụ cơ bản**
  ```bash
  git clone https://github.com/anhnth24/project-example.git
  cd project-example
  make check-toolchain
  ```
  *Yêu cầu*: Rust 1.88+, Docker Engine, Compose v2, Python 3.12+, Node 20+ và pnpm 10.33.3.

- [ ] **Bước 2: Cài đặt dependencies và chuẩn bị PDFium**
  ```bash
  make install
  bash bench/download_pdfium.sh
  ```

- [ ] **Bước 3: Khởi tạo file cấu hình môi trường từ mẫu**
  ```bash
  cp deploy/.env.example deploy/.env
  # File deploy/.env đã được gitignore, tuyệt đối không commit file này!
  ```

- [ ] **Bước 4: Chạy thử nghiệm Mock POC Stack (Zero-config / Không cần API Key)**
  ```bash
  COMPOSE_PROFILES=mock deploy/scripts/up.sh
  ```
  Xác nhận toàn bộ dịch vụ phụ trợ (Postgres, Qdrant, MinIO, Mock-Embedding) đều `healthy`.

- [ ] **Bước 5: Thử nghiệm CLI OCR ở chế độ Fail-closed**
  ```bash
  cargo build --release -p fileconv-cli
  ./target/release/fileconv one bench/markhand_web/golden/documents/gold-020.png
  ```
  Xác nhận lệnh báo lỗi `vision OCR chưa cấu hình` (chứng minh hệ thống không nuốt lỗi âm thầm).

- [ ] **Bước 6: Cấu hình CLI với OpenRouter Key để kiểm tra OCR độc lập**
  ```bash
  export FILECONV_OCR_API_KEY="sk-or-v1-..."  # Hoặc dùng FILECONV_LLM_API_KEY
  ./target/release/fileconv one bench/markhand_web/golden/documents/gold-020.png
  ```
  Xác nhận nội dung Markdown của ảnh được in ra màn hình chính xác.

- [ ] **Bước 7: Tạo Index Signature cho OpenRouter Cloud Embedding**
  Chạy lệnh sinh chữ ký tương thích với model Qwen 8B:
  ```bash
  python3 deploy/scripts/print-index-signature.py \
    --base-url https://openrouter.ai/api/v1 \
    --model qwen/qwen3-embedding-8b \
    --revision qwen3-embedding-8b-20251028 \
    --dimensions 1024
  ```
  Kết quả sinh ra chuỗi SHA256: `229680cc2d8df20a0776d3c06b31f88a9d0f201f2047b3849e2c9ea47545629f`.


- [ ] **Bước 8: Cập nhật cấu hình OpenRouter vào `deploy/.env`**
  Mở `deploy/.env`, thay thế block Mock Embedding, Chat và Vision OCR bằng các biến OpenRouter:
  ```bash
  # Cloud Embedding
  MARKHAND_EMBEDDING_BASE_URL=https://openrouter.ai/api/v1
  MARKHAND_EMBEDDING_API_KEY=sk-or-v1-...
  MARKHAND_EMBEDDING_PROVIDER=openrouter
  MARKHAND_EMBEDDING_MODEL=qwen/qwen3-embedding-8b
  MARKHAND_EMBEDDING_REVISION=qwen3-embedding-8b-20251028
  MARKHAND_EMBEDDING_DIMENSIONS=1024
  MARKHAND_EMBEDDING_RUNTIME_PATH=provider-cloud
  MARKHAND_ALLOW_CLOUD_EMBEDDINGS=true
  MARKHAND_EMBEDDING_NORMALIZE=client
  MARKHAND_EMBEDDING_SEND_DIMENSIONS=true
  MARKHAND_INDEX_SIGNATURE=229680cc2d8df20a0776d3c06b31f88a9d0f201f2047b3849e2c9ea47545629f

  # Readiness probe timeout
  MARKHAND_READY_PROBE_TIMEOUT_SECS=10

  # Grounded Chat
  MARKHAND_CHAT_BASE_URL=https://openrouter.ai/api/v1
  MARKHAND_CHAT_API_KEY=sk-or-v1-...
  MARKHAND_CHAT_MODEL=qwen/qwen3.7-flash
  MARKHAND_CHAT_REASONING=off

  # Worker Vision OCR
  MARKHAND_OCR_API_KEY=sk-or-v1-...
  MARKHAND_OCR_BASE_URL=https://openrouter.ai/api
  MARKHAND_OCR_MODEL=qwen/qwen3.7-flash
  MARKHAND_OCR_BATCH_PAGES=5
  ```

- [ ] **Bước 9: Khởi động lại Server & Workers với cấu hình mới**
  ```bash
  deploy/scripts/poc-up.sh
  # Hoặc chạy backend binary trên host:
  # cargo run -p fileconv-server
  ```
  Kiểm tra probe sẵn sàng: `curl http://127.0.0.1:8788/api/v1/health/ready`.

- [ ] **Bước 10: Rà soát an toàn bảo mật (Secret Sanitization Check)**
  - Kiểm tra `git status` đảm bảo không có file secret hoặc file `.env` nào bị track:
    ```bash
    git status --ignored
    ```
  - Chạy script kiểm tra vi phạm bảo mật:
    ```bash
    python3 deploy/scripts/redact_secrets.py
    ```

---

## 7. Quy Tắc An Toàn Bảo Mật (Secret Hygiene)

1. **Không commit API Key**: Bất kỳ API key thật nào từ OpenRouter tuyệt đối không được đưa vào git commit, test fixture, hoặc issue comment.
2. **Không nới lỏng Sandbox**: Converter sandbox bắt buộc duy trì trạng thái cách ly mạng `CLONE_NEWNET`. Không bao giờ truyền `MARKHAND_OCR_API_KEY` hay `FILECONV_OCR_API_KEY` vào trong môi trường sandbox của worker.
3. **Redact Logs**: Toàn bộ log của server và worker phải tự động che giấu (redact) key và prompt; lỗi kết nối chỉ được ghi nhận mã trạng thái và độ trễ, không in header `Authorization`.
