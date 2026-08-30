# Báo cáo Deep-dive: OpenRouter Embedding Pipeline (Task inter-v2-03)

- **Mục tiêu:** Trace chi tiết luồng xử lý embedding indexing từ text chunking đến OpenRouter `qwen/qwen3-embedding-8b` và lưu trữ vector vào Qdrant.
- **Tài liệu tham chiếu:**
  - `docs/adr/0016-openrouter-qwen-ocr-embedding.md`
  - `docs/adr/0005-vietnamese-embedding-model-quality.md`
  - `bench/markhand_web/reports/openrouter-embedding-evaluation.md` (Benchmark DKP-02)
  - `crates/server/src/services/embedding.rs`
  - `crates/server/src/workers/embedding.rs`
  - `crates/knowledge/src/embedding.rs`
  - `deploy/.env.example`

---

## 1. Tổng quan luồng Index → Embed → Upsert Qdrant

Toàn bộ quy trình embedding được thiết kế theo cơ chế worker bền vững (durable worker), đảm bảo backpressure, tính toàn vẹn dữ liệu (data integrity), kiểm soát egress bảo mật và chống trộn lẫn index generation (ADR 0006/0011).

```
[Markdown Document]
        │
        ▼
[Chunking Engine] (heading-chunks-2000-v1)
        │
        ├──► PostgreSQL: Chunks Table (Lưu chunk text + chunk_identity_sha256)
        └──► PostgreSQL: Jobs Table (JobType::EmbeddingBatch)
                    │
                    ▼
          [EmbeddingWorker] (Lease lock + Heartbeat)
                    │
                    ├─ 1. Load Chunks & Validate Checksum (canonical_inputs_sha256)
                    ├─ 2. Validate Index Signature Generation
                    │
                    ▼
        [ApprovedEmbeddingRuntime]
                    │
                    ├─ Payload Format: "{heading}\n{text}"
                    ├─ HTTP POST https://openrouter.ai/api/v1/embeddings
                    ├─ API Key Auth (Bearer token)
                    ├─ Request Dimensions: 1024 (MRL)
                    │
                    ▼
          [Client-side Validation]
                    ├─ Parse & Sort Response Vectors
                    ├─ L2 Normalization (normalize_client = true)
                    ├─ Verify Dimensions == Expected Dimensions
                    │
                    ▼
         [Vector Storage: Qdrant]
                    ├─ Ensure Collection for Signature
                    ├─ Deterministic Point ID: (org_id, collection_id, chunk_hash)
                    ├─ PostgreSQL Fenced Txn: Cleanup Intent
                    └─ gRPC/HTTP Upsert Point (Vector + Payload)
```

### Chi tiết các giai đoạn:

1. **Giai đoạn 1: Chuẩn bị Chunks và Enqueue Job (Indexing Pipeline)**
   - Tài liệu sau khi convert/OCR được chia thành các đoạn văn bản (chunks) theo phiên bản chunking `heading-chunks-2000-v1`.
   - Các chunk được gán định danh duy nhất `chunk_identity_sha256` và lưu vào bảng `chunks`.
   - Hệ thống tạo một bản ghi `EmbeddingBatch` đại diện cho một dải chunk (bounded range) kèm mã băm đầu vào `input_sha256` và kích hoạt một job thuộc loại `JobType::EmbeddingBatch`.

2. **Giai đoạn 2: Claiming và Kiểm tra Tính toàn vẹn (EmbeddingWorker)**
   - `EmbeddingWorker` thực hiện `jobs::claim_type` để lấy quyền xử lý job thông qua lease token có thời hạn (`lease_ttl`, heartbeat định kỳ).
   - Worker tái tạo danh sách chuỗi đầu vào theo quy ước chuẩn:
     $$\text{canonical\_input} = \text{heading\_path.join(" > ")} + \text{"\textbackslash n"} + \text{body}$$
   - Tính toán `canonical_inputs_sha256(&inputs)` và so sánh với `batch.input_sha256` trong cơ sở dữ liệu. Nếu không khớp $\rightarrow$ `InputChecksumMismatch` (fail-closed, từ chối gửi dữ liệu sai).
   - Kiểm tra `validate_target_generation`: Xác minh target index generation giữa DB metadata và `ApprovedEmbeddingRuntime` để cấm tuyệt đối việc trộn lẫn vector giữa các đời model/dimension (ADR 0011).

3. **Giai đoạn 3: Dispatch Payload sang OpenRouter (ApprovedEmbeddingRuntime)**
   - Worker gọi `self.runtime.embed(&inputs)`.
   - `ApprovedEmbeddingRuntime` tạo payload JSON tuân thủ chuẩn OpenAI-compatible:
     - `model`: `"qwen/qwen3-embedding-8b"`
     - `input`: danh sách các chuỗi `{heading}\n{text}`
     - `encoding_format`: `"float"`
     - `dimensions`: `1024` (nếu `MARKHAND_EMBEDDING_SEND_DIMENSIONS=true` cho Matryoshka Representation Learning).
   - Gửi `POST` tới endpoint `https://openrouter.ai/api/v1/embeddings` kèm header `Authorization: Bearer <API_KEY>`.

4. **Giai đoạn 4: Chuẩn hóa Vector và Xác thực (Client-side Normalization)**
   - `ApprovedEmbeddingRuntime` nhận response `ProviderResponse` chứa mảng các vector kèm `index`.
   - Các vector được sắp xếp theo đúng `index` ban đầu để tránh xáo trộn thứ tự chunk.
   - **Client-side Normalization:** Do OpenRouter và một số upstream gateway không đảm bảo vector trả về đã được chuẩn hóa L2 (unit vector), cờ `MARKHAND_EMBEDDING_NORMALIZE=client` sẽ kích hoạt hàm chuẩn hóa client-side:
     $$v_{\text{norm}} = \frac{v}{\|v\|_2} = \frac{v}{\sqrt{\sum_{i=1}^n v_i^2}}$$
   - Kiểm tra độ dài vector: Nếu kích thước không khớp chính xác với `expected_dimensions` (1024-d hoặc 4096-d) $\rightarrow$ `SignatureMismatch`.

5. **Giai đoạn 5: Upsert vào Qdrant & Transaction Fencing**
   - Đảm bảo collection Qdrant cho signature hiện tại đã tồn tại (`ensure_collection_for_signature`).
   - Tạo `PointId` dạng UUID v5 tất định từ bộ ba `(org_id, collection_id, chunk_identity_sha256)` nhằm đảm bảo tính idempotent khi retry.
   - Ghi nhận `vector_cleanup_intents` trong cùng transaction PostgreSQL kiểm tra document lifecycle trước khi gọi ra bên ngoài.
   - Upsert vector cùng metadata (`ChunkPointPayload` gồm org_id, document_id, version_id, chunk_index, heading, text preview, v.v.) vào Qdrant.
   - Đánh dấu `Job` hoàn tất (`Completed`).

---

## 2. Trace 1 Job Embedding Điển hình

### 2.1. Request gửi đi (HTTP Payload)
- **Endpoint:** `POST https://openrouter.ai/api/v1/embeddings`
- **Headers:**
  ```http
  POST /api/v1/embeddings HTTP/1.1
  Host: openrouter.ai
  Authorization: Bearer sk-or-v1-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
  Content-Type: application/json
  ```
- **JSON Body:**
  ```json
  {
    "model": "qwen/qwen3-embedding-8b",
    "input": [
      "Quy định an toàn lao động > Điều 1: Phạm vi áp dụng\nQuy định này áp dụng cho toàn thể cán bộ nhân viên trong công ty...",
      "Quy định an toàn lao động > Điều 2: Trách nhiệm người lao động\nNgười lao động có trách nhiệm tuân thủ đầy đủ các trang thiết bị bảo hộ..."
    ],
    "encoding_format": "float",
    "dimensions": 1024
  }
  ```

### 2.2. Response nhận về từ OpenRouter
- **HTTP Status:** `200 OK`
- **JSON Body:**
  ```json
  {
    "object": "list",
    "data": [
      {
        "object": "embedding",
        "index": 0,
        "embedding": [0.01234, -0.04567, 0.08910, "... (1024 phần tử float) ..."]
      },
      {
        "object": "embedding",
        "index": 1,
        "embedding": [-0.03210, 0.06543, -0.01234, "... (1024 phần tử float) ..."]
      }
    ],
    "model": "qwen/qwen3-embedding-8b",
    "usage": {
      "prompt_tokens": 128,
      "total_tokens": 128
    }
  }
  ```

### 2.3. Client-side Normalization & Validation
1. **Kiểm tra độ dài vector:** Xác nhận mỗi vector trong mảng `data` có đúng `1024` chiều.
2. **Tính chuẩn Euclidean ($L_2$ norm):**
   $$\|v\|_2 = \sqrt{\sum_{i=1}^{1024} v_i^2}$$
3. **Thực hiện chuẩn hóa:**
   $$v'_i = \frac{v_i}{\|v\|_2}$$
   *(Nếu $\|v\|_2 \approx 0$ hoặc không hợp lệ, hệ thống lập tức fail-closed với lỗi `InvalidInput`).*
4. **Kiểm tra sau chuẩn hóa:** Đảm bảo $\|v'\|_2 \approx 1.0 \pm 10^{-4}$.

### 2.4. Upsert Payload vào Qdrant
- **Collection Name:** `markhand_org_<org_id>_<signature_digest_prefix>`
- **Point Structure:**
  ```json
  {
    "id": "e4d9b231-8f52-5a23-b671-123456789abc",
    "vector": [0.01234, -0.04567, 0.08910, "... (vector đã chuẩn hóa L2) ..."],
    "payload": {
      "org_id": "11111111-1111-1111-1111-111111111111",
      "document_id": "33333333-3333-3333-3333-333333333333",
      "version_id": "44444444-4444-4444-4444-444444444444",
      "collection_id": "55555555-5555-5555-5555-555555555555",
      "chunk_identity_sha256": "a1b2c3d4...",
      "ordinal": 0,
      "heading_path": ["Quy định an toàn lao động", "Điều 1: Phạm vi áp dụng"],
      "is_current": true,
      "is_effective": true
    }
  }
  ```


---

## 3. So sánh Cấu hình Môi trường: Mock POC vs OpenRouter Thật

| Biến môi trường (`.env`) | Mock POC (Môi trường Dev/Test) | OpenRouter Cloud (Môi trường Thực tế) | Ý nghĩa / Ảnh hưởng kiến trúc |
|---|---|---|---|
| `COMPOSE_PROFILES` | `mock` | `""` (không cần mock container) | Xác định profile dịch vụ khởi chạy trong Docker Compose. |
| `MARKHAND_EMBEDDING_BASE_URL` | `http://mock-embedding:8080/v1` | `https://openrouter.ai/api/v1` | Endpoint tiếp nhận OpenAI-compatible embedding API. |
| `MARKHAND_EMBEDDING_API_KEY` | `poc-embedding-key` | `sk-or-v1-xxxxxxxx...` | API Key xác thực. (Tuyệt đối không commit key thật lên git). |
| `MARKHAND_EMBEDDING_PROVIDER` | `openai-compatible` | `openrouter` (hoặc `openai-compatible`) | Phân loại provider cho index signature calculation. |
| `MARKHAND_EMBEDDING_MODEL` | `markhand-mock` | `qwen/qwen3-embedding-8b` | Tên định danh model được gọi qua API. |
| `MARKHAND_EMBEDDING_REVISION` | `poc-local` | `qwen3-embedding-8b-20251028` (hoặc observed date pin) | Revision gắn vào Index Signature để phát hiện model drift. |
| `MARKHAND_EMBEDDING_DIMENSIONS`| `8` | `1024` (hoặc `4096`) | Kích thước không gian vector lưu trong Qdrant. |
| `MARKHAND_EMBEDDING_RUNTIME_PATH` | `local-neural` | `provider-cloud` | Đường dẫn runtime. Giá trị `provider-cloud` yêu cầu opt-in tường minh. |
| `MARKHAND_ALLOW_CLOUD_EMBEDDINGS` | Không bắt buộc (hoặc `false`) | **`true` (BẮT BUỘC)** | **Policy guardrail:** Cho phép gửi dữ liệu chunk ra cloud. |
| `MARKHAND_EMBEDDING_NORMALIZE` | `provider` / unset | `client` | Kích hoạt L2 normalization tại worker trước khi lưu Qdrant. |
| `MARKHAND_EMBEDDING_SEND_DIMENSIONS` | `false` / unset | `true` | Yêu cầu MRL dimension trong JSON payload gửi OpenRouter. |
| `MARKHAND_INDEX_SIGNATURE` | Signature tính theo `markhand-mock` (8-d) | Signature tính theo `qwen3-8b` + OpenRouter URL (1024-d) | Khóa digest của toàn bộ tham số index; sai signature sẽ từ chối job. |

---

## 4. Tại sao cần `MARKHAND_ALLOW_CLOUD_EMBEDDINGS` tường minh?

### 4.1. Khác biệt giữa RAG Q&A Egress và Indexing Egress
Trong kiến trúc bảo mật của Markhand:
1. **Giai đoạn Chat / Q&A (LLM Egress):** Hệ thống chỉ gửi câu hỏi của người dùng kèm một vài đoạn trích dẫn ngắn đã được truy xuất (Top-K context slices, thường < 5 chunks) tới LLM.
2. **Giai đoạn Indexing (Embedding Egress):** Hệ thống phải gửi **100% toàn bộ nội dung tài liệu nguyên bản (full raw text chunks)** qua API để tính toán vector.

### 4.2. Bài toán bảo mật từ ADR 0005 sang ADR 0016
- **ADR 0005 (Baseline ban đầu):** Đã lựa chọn mô hình chạy nội bộ `AITeamVN/Vietnamese_Embedding` (`local-neural` trên CPU/GPU on-premise) chính vì mục tiêu **bảo vệ dữ liệu khách hàng tuyệt đối không bị egress ra ngoài đám mây** trong quá trình lập chỉ mục.
- **ADR 0016 (Chuyển đổi sang OpenRouter):** Khi áp dụng OpenRouter cho Markhand Web để tối ưu chi phí vận hành và không phụ thuộc GPU on-premise, toàn bộ văn bản của khách hàng sẽ đi qua bên thứ ba (OpenRouter gateway + upstream inference provider).
- **Cơ chế Fail-closed Guardrail:** Cờ `MARKHAND_ALLOW_CLOUD_EMBEDDINGS=true` đóng vai trò là một chốt an toàn bắt buộc (explicit opt-in policy).
  - Nếu `MARKHAND_EMBEDDING_RUNTIME_PATH=provider-cloud` nhưng biến `MARKHAND_ALLOW_CLOUD_EMBEDDINGS` bị thiếu hoặc bằng `false`, hàm `validate_runtime_policy()` trong `crates/server/src/services/embedding.rs` sẽ lập tức trả về lỗi `EmbeddingError::CloudRuntimeNotAllowed`.
  - Cơ chế này đảm bảo không bao giờ có chuyện vô tình rò rỉ tài liệu mật/nội bộ lên cloud khi triển khai ở các môi trường on-premise hoặc air-gapped mà chưa có sự phê duyệt có chủ đích từ người quản trị hệ thống.


---

## 5. Danh sách Biến Môi trường Bắt buộc cho OpenRouter Embedding

Để kích hoạt OpenRouter embedding trong môi trường dev/staging/production, bắt buộc phải khai báo đầy đủ các biến sau:

```bash
# 1. Endpoint OpenRouter API v1
MARKHAND_EMBEDDING_BASE_URL=https://openrouter.ai/api/v1

# 2. Khóa xác thực bí mật (Secret - không commit git)
MARKHAND_EMBEDDING_API_KEY=sk-or-v1-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx

# 3. Phân loại provider
MARKHAND_EMBEDDING_PROVIDER=openrouter

# 4. Tên model embedding Qwen được hỗ trợ
MARKHAND_EMBEDDING_MODEL=qwen/qwen3-embedding-8b

# 5. Model revision pin (ngăn chặn silent model drift từ provider)
MARKHAND_EMBEDDING_REVISION=qwen3-embedding-8b-20251028

# 6. Chiều vector mong muốn (1024 cho Matryoshka hoặc 4096 cho Native)
MARKHAND_EMBEDDING_DIMENSIONS=1024

# 7. Định danh đường dẫn cloud runtime
MARKHAND_EMBEDDING_RUNTIME_PATH=provider-cloud

# 8. Cờ đồng thuận egress dữ liệu bắt buộc (Fail-closed nếu thiếu)
MARKHAND_ALLOW_CLOUD_EMBEDDINGS=true

# 9. Chuẩn hóa L2 phía client (OpenRouter không đảm bảo unit vector)
MARKHAND_EMBEDDING_NORMALIZE=client

# 10. Gửi tham số dimension trong request payload (hỗ trợ MRL)
MARKHAND_EMBEDDING_SEND_DIMENSIONS=true

# 11. Chữ ký index tính toán đồng bộ (dùng print-index-signature.py)
MARKHAND_INDEX_SIGNATURE=72dda20007ffb7fbe293612091103321eb9e4e0e4a0517a5f3413e31a2978874
```

---

## 6. Phân tích Trade-off: 1024-d vs 4096-d (Benchmark DKP-02)

Dựa trên kết quả đo kiểm thực nghiệm tại `bench/markhand_web/reports/openrouter-embedding-evaluation.md`:

### 6.1. Bảng số liệu Benchmark DKP-02

| Cấu hình Model | Dims | Recall@5 (min) | Hit@5 | MRR | nDCG@10 (min) | Recall $\ge 0.85$ | nDCG Gap $\le 0.02$ |
|---|---:|---:|---:|---:|---:|:---:|:---:|
| `qwen/qwen3-embedding-8b` (Native) | **4096** | **0.9436** | 0.9538 | 0.7737 | **0.8072** | **PASS** | **PASS (0.0000)** |
| `qwen/qwen3-embedding-8b` (MRL) | **1024** | **0.9181** | 0.9286 | 0.7577 | **0.7942** | **PASS** | **PASS (0.0152)** |
| *Baseline cũ: AITeamVN local (ADR 0005)* | *1024* | *0.9261* | *0.9320* | *0.7610* | *0.7990* | *PASS* | *Baseline* |

### 6.2. Phân tích Chi tiết Trade-off

1. **Chất lượng truy xuất (Retrieval Quality):**
   - **4096-d (Native):** Đạt chất lượng truy xuất cao nhất trên tập benchmark tiếng Việt (`Recall@5 = 0.9436`, `nDCG@10 = 0.8072`). Giữ trọn vẹn không gian biểu diễn ngữ nghĩa của mô hình 8 tỷ tham số.
   - **1024-d (Matryoshka / MRL):** Nhờ cơ chế huấn luyện Matryoshka Representation Learning, việc cắt ngắn (truncate) từ 4096 xuống 1024 chiều chỉ làm giảm nhẹ ~2.5% Recall@5 (`0.9181` so với `0.9436`). Mức điểm này vẫn vượt xa ngưỡng chất lượng của dự án (`0.85`) và nDCG gap đạt `0.0152` (nằm trong giới hạn cho phép $\le 0.02$).

2. **Chi phí Hạ tầng & Bộ nhớ (Resource & Infrastructure Cost):**
   - **Tiết kiệm 75% RAM và Disk trên Qdrant:** Lưu trữ 1 triệu vector ở 4096-d (float32) tiêu tốn khoảng **16.38 GB RAM thô** (chưa tính HNSW index overhead). Trong khi đó, ở 1024-d chỉ tốn khoảng **4.09 GB RAM**, giúp giảm 4 lần chi phí server vector database.
   - **Tốc độ tính toán ANN Search:** Vector 1024-d giảm đáng kể số phép tính SIMD dot-product/cosine trong quá trình tìm kiếm k-NN lân cận, cải thiện p99 search latency và tăng throughput truy vấn đồng thời.

3. **Kết luận kiến trúc:**
   - Cấu hình **1024-d** là lựa chọn tối ưu (sweet spot) được đề xuất cho Markhand Web: vừa tiết kiệm 75% chi phí lưu trữ/tính toán, vừa đảm bảo vượt qua toàn bộ quality gates khắt khe của hệ thống.

