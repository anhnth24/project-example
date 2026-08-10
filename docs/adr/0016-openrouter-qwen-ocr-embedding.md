# ADR 0016: OpenRouter Qwen cho OCR và embedding của Markhand Web

- Status: Accepted (phần OCR — quyết định trực tiếp của product owner
  2026-08-10, đã implement); phần embedding cutover vẫn gated theo điều kiện
  bên dưới
- Date: 2026-08-10
- Owners: retrieval-owner, worker-owner
- Approver: product-owner (OCR: approved 2026-08-10; embedding: pending
  benchmark evidence)
- Supersedes: [ADR 0005](0005-vietnamese-embedding-model-quality.md) (embedding
  runtime selection cho Markhand Web — khi phần embedding được kích hoạt)
- Related issues/PRs: draft DKP-01…DKP-07 trong
  [`brainstorm-260810-1456-document-knowledge-platform-openrouter-report.md`](../../plans/reports/brainstorm-260810-1456-document-knowledge-platform-openrouter-report.md)

## Context

Product owner quyết định dùng OpenRouter cho Markhand Web: `qwen/qwen3.7-flash`
(vision) làm primary OCR cho PDF scan và `qwen/qwen3-embedding-8b` làm embedding
model, thay cho stack local hiện tại. Hiện trạng và ràng buộc:

- ADR 0005 (Accepted) pin embedding web vào `AITeamVN/Vietnamese_Embedding` local
  (1024-d, `local-neural`, Recall@5 = 0.9261) với lý do chính: index build gửi toàn
  bộ chunk text — không cho cloud egress. Server code enforce: `provider-cloud`
  chỉ được phép ở profile `dev` + `MARKHAND_ALLOW_CLOUD_EMBEDDINGS`.
- Convert pipeline chỉ có Tesseract/Paddle (`image_ocr.rs`); `vision_ocr` tồn tại
  nhưng chỉ là tool opt-in (`ocr_hard`), không nối vào PDF `needs_ocr`. Sandbox
  converter không có network.
- ADR 0006/0011: đổi model/dimension/runtime_path/chunking ⇒ index generation mới,
  expand → shadow → cutover, cấm trộn generation.
- Xác minh 2026-08-10: OpenRouter có `POST /api/v1/embeddings`;
  `qwen/qwen3-embedding-8b` 4096-d native (MRL 32–4096), ~$0.01/M token;
  `qwen/qwen3.7-flash` (snapshot `qwen3.7-flash-20260727`) vision, $0.03/M input,
  nhưng eval OCR độc lập xếp hạng chót (84.1% Roboflow) và chỉ có một upstream
  provider — trái với trải nghiệm nội bộ tốt trên tài liệu tiếng Việt.

## Decision

1. **OCR (Accepted, đã implement 2026-08-10):** vision-LLM qua OpenRouter là
   **đường OCR duy nhất**; **Tesseract/PaddleOCR local bị loại bỏ hoàn toàn**
   theo chỉ đạo trực tiếp của product owner (không giữ fallback local).
   `pdf-inspector` tiếp tục quyết định trang nào cần OCR — trang có text layer
   tin cậy không đi qua OCR. Trang `needs_ocr`/ảnh được render (PDFium 300 DPI),
   thu về ≤2400px, encode JPEG q90 rồi gửi provider (Qwen chỉ nhận ảnh). Output
   giữ canonical page contract (`<!-- Trang N (OCR) -->`); system prompt là hợp
   đồng "chép trung thực" tổng quát cho cả văn bản pháp luật lẫn tài liệu dự án
   (giữ số hiệu/điều-khoản-điểm/mã định danh/bảng/công thức/code; `[không rõ:
   ...]` khi không chắc; chỉ xuất Markdown) — override được qua
   `FILECONV_OCR_SYSTEM_PROMPT`. Reasoning tắt trong request OCR. Thiếu
   key/endpoint → `DependencyMissing` fail-closed, không âm thầm bỏ trang.
   Cấu hình: `FILECONV_OCR_API_KEY` (fallback `FILECONV_LLM_API_KEY`),
   `FILECONV_OCR_BASE_URL` (mặc định OpenRouter; endpoint local vLLM/Ollama
   vision là phương án offline), `FILECONV_OCR_MODEL` (mặc định
   `qwen/qwen3.7-flash`), `FILECONV_OCR_TIMEOUT_SECS`.
   **Server (đã implement — deferred OCR):** converter sandbox giữ nguyên
   no-network và không bao giờ nhận API key; sandbox chỉ render JPEG trang
   `needs_ocr` vào workspace (`fileconv one --ocr-defer-dir .`) kèm placeholder
   `markhand:ocr-pending`; worker tin cậy thu artifact (cap 512 trang / 8MB /
   256MB), gọi provider qua `MARKHAND_OCR_*` rồi thay placeholder. Thiếu config
   → job fail với `vision OCR not configured`, retry/backoff → dead-letter.
   Compose POC: worker-convert thêm network `ocr-egress` riêng (sandbox không
   thấy được — CLONE_NEWNET).
2. **Embedding (đã implement, chờ benchmark để cutover mặc định):** Markhand
   Web hỗ trợ `qwen/qwen3-embedding-8b` qua OpenRouter
   (`runtime_path=provider-cloud`). Policy: `provider-cloud` được phép ở mọi
   profile **chỉ khi** deployment bật cờ egress tường minh
   `MARKHAND_ALLOW_CLOUD_EMBEDDINGS=true`; thêm
   `MARKHAND_EMBEDDING_NORMALIZE=client` (normalize server-side rồi verify) và
   `MARKHAND_EMBEDDING_SEND_DIMENSIONS=true` (MRL 4096→1024). Đã probe thực tế
   2026-08-10: OpenRouter trả 4096-d mặc định, `dimensions:1024` hoạt động,
   vector đã L2-normalized. Profile air-gapped giữ `local-neural` (AITeamVN)
   như một generation riêng.
3. **Model pin:** model slug + revision/snapshot + dimension override được qua
   deployment config (env); mọi giá trị pin vào index signature (embedding) và
   ghi vào observability job (OCR).
4. **Điều kiện kích hoạt phần embedding (fail-closed):**
   - benchmark embedding trên golden corpus vi bằng harness ADR 0005:
     Recall@5 ≥ 0.85 (min 3 runs) và so sánh trực tiếp với AITeamVN 0.9261;
     quyết định dimension (4096 vs 1024 MRL) kèm số liệu;
   - security review cho secrets/egress + LLM content policy;
   - index generation migration theo ADR 0011 (expand → shadow → cutover).
   Khuyến nghị bổ sung cho OCR (không chặn, vì đã Accepted): đo CER/WER trên
   corpus scan vi bằng `fileconv accuracy` để có baseline chất lượng so với số
   liệu Tesseract cũ trong `bench/REPORT_ACCURACY.md`.

## Consequences

- Positive: OCR cấu trúc (bảng/form/đa cột/reading order) tốt hơn Tesseract trên
  tài liệu khó; embedding đa ngôn ngữ mạnh, không cần GPU on-prem; bỏ được service
  `embedding-cpu` cho deployment cloud-allowed; chi phí rebuild toàn corpus thấp
  (~$0.01/M token).
- Negative / security: toàn bộ ảnh trang scan và chunk text đi qua OpenRouter →
  thay đổi hẳn tư thế egress mà ADR 0005 bảo vệ; yêu cầu policy per-deployment và
  khả năng opt-out per-org (thiết kế ở DKP-06).
- Negative: phụ thuộc availability/rate-limit bên thứ ba trên đường ingest;
  Qwen3.7 Flash chỉ có một upstream provider.
- Migration: index generation mới theo ADR 0011 (expand → backfill → shadow →
  cutover); nếu chunking version đổi trong cùng giai đoạn thì gộp một generation.
- Roadmap/gates: gate `G0-RET-VLLM-CUTOVER` retired khỏi `gates.yaml`
  (2026-08-10); `G0-RET-RECALL-AT-5`/`G0-RET-BEST-MODEL-GAP` giữ nguyên và áp
  cho benchmark OpenRouter (DKP-02). Tech stack/quyết định mở trong
  `plans/markhand-web/README.md` đã cập nhật tương ứng.
- Operational: job observability phải ghi engine/token/cost; key qua secret mount,
  redacted, không log.

## Alternatives considered

- **Giữ ADR 0005 (AITeamVN local):** chất lượng đã vượt gate, không egress; nhưng
  product direction chọn cloud để bỏ ràng buộc hạ tầng và thống nhất provider.
  Vẫn giữ làm profile air-gapped.
- **vLLM GPU on-prem (target cũ của ADR 0005):** không còn là target bắt buộc;
  chi phí GPU và vận hành cao hơn OpenRouter cho quy mô hiện tại.
- **Giữ Tesseract làm fallback per-page:** bị product owner loại bỏ trực tiếp
  (2026-08-10) — đơn giản hoá stack, bỏ bundle native runtime; offline dùng
  endpoint vision local thay thế.
- **OCR bằng tier cao hơn (Qwen3.7 Plus / Qwen3-VL):** giữ làm ứng viên đối chứng
  trong benchmark DKP-03 (đo hậu kiểm chất lượng); Flash được chọn vì cost.
- **Đục network cho sandbox để OCR trong convert stage:** bị loại — phá invariant
  cô lập converter; stage riêng ngoài sandbox giữ nguyên threat model upload.

## Verification

```bash
# Embedding gate (harness ADR 0005, mở rộng API runtime)
python3 bench/markhand_web/scripts/run_embedding_eval.py --runs 3
python3 bench/markhand_web/scripts/run_retrieval_eval.py

# OCR baseline chất lượng (hậu kiểm)
./target/release/fileconv accuracy <scan-manifest.tsv> bench/REPORT_OCR_VISION.md

# OCR fail-closed + prompt contract (đã có trong repo)
cargo test -p fileconv-core --features llm image_ocr
cargo test -p fileconv-core --features llm conv::pdf

# Policy/config
cargo test -p fileconv-server embedding   # runtime_path/policy/normalize-client
cargo test -p fileconv-knowledge --lib identity::tests
```

Denial/negative bắt buộc (OCR đã có test): thiếu key/feature →
`DependencyMissing` fail-closed; trang text-layer rác giữ untrusted text +
warning (PartialSuccess) thay vì bịa nội dung; sandbox không nhận OCR API key
(poc-isolation-smoke). Embedding: lỗi → job pending/backoff, không mất dữ liệu,
không trộn generation; signature mismatch từ chối retrieval; key không xuất
hiện trong log.

## Exception lifecycle

| Field | Value |
|---|---|
| Exception | Air-gapped/offline deployment không dùng OpenRouter |
| Owner | operations-owner |
| Scope | OCR = endpoint vision local (`FILECONV_OCR_BASE_URL`) hoặc không OCR (chỉ tài liệu có text layer); embedding = `local-neural` generation riêng |
| Expiry | Không hết hạn — là mode được hỗ trợ chính thức |
| Retest | Mỗi release: e2e ingest không egress ngoài endpoint local pass |
