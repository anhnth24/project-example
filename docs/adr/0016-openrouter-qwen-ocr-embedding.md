# ADR 0016: OpenRouter Qwen cho OCR và embedding của Markhand Web

- Status: Proposed
- Date: 2026-08-10
- Owners: retrieval-owner, worker-owner
- Approver: product-owner (pending)
- Supersedes: [ADR 0005](0005-vietnamese-embedding-model-quality.md) (embedding
  runtime selection cho Markhand Web — chỉ khi ADR này được Accepted kèm evidence)
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

1. **OCR:** vision-LLM qua OpenRouter là primary OCR path cho trang `needs_ocr`
   trên Markhand Web; Tesseract local là fallback bắt buộc (per-page, tự động) và
   là engine duy nhất cho deployment air-gapped/offline. Cả hai engine xuất cùng
   canonical Markdown page contract (`<!-- Trang N (OCR) -->`); downstream không
   phụ thuộc engine. Vision OCR chạy ở worker stage ngoài sandbox (sandbox vẫn
   không có network); egress allowlist `openrouter.ai`; request đặt
   `provider.data_collection: deny`.
2. **Embedding:** Markhand Web dùng `qwen/qwen3-embedding-8b` qua OpenRouter
   (`runtime_path=provider-cloud`) cho index build và query. Server bỏ hạn chế
   dev-only cho `provider-cloud` **chỉ khi** deployment bật flag cho phép cloud
   embeddings và khai báo data classification tương thích; profile air-gapped giữ
   `local-neural` (AITeamVN) như một generation riêng.
3. **Model pin:** model slug + revision/snapshot + dimension nằm trong deployment
   config (env), không hardcode trong code; mọi giá trị pin vào index signature.
4. **Điều kiện Accepted (fail-closed):** ADR này chỉ chuyển Accepted khi có đủ:
   - benchmark embedding trên golden corpus vi bằng harness ADR 0005:
     Recall@5 ≥ 0.85 (min 3 runs) và so sánh trực tiếp với AITeamVN 0.9261;
     quyết định dimension (4096 vs 1024 MRL) kèm số liệu;
   - benchmark OCR CER/WER trên corpus scan vi: Qwen3.7 Flash vs Tesseract vs ≥1
     VLM đối chứng; vision chỉ thành primary khi thắng Tesseract có số liệu;
   - security review cho secrets/egress + LLM content policy;
   - product-owner approval bằng văn bản trên hai kết quả trên.

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
- Operational: job observability phải ghi engine/token/cost; key qua secret mount,
  redacted, không log.

## Alternatives considered

- **Giữ ADR 0005 (AITeamVN local):** chất lượng đã vượt gate, không egress; nhưng
  product direction chọn cloud để bỏ ràng buộc hạ tầng và thống nhất provider.
  Vẫn giữ làm profile air-gapped.
- **vLLM GPU on-prem (target cũ của ADR 0005):** không còn là target bắt buộc;
  chi phí GPU và vận hành cao hơn OpenRouter cho quy mô hiện tại.
- **OCR bằng tier cao hơn (Qwen3.7 Plus / Qwen3-VL):** giữ làm ứng viên đối chứng
  trong benchmark DKP-03; Flash được chọn trước vì cost, nhưng không được miễn gate.
- **Đục network cho sandbox để OCR trong convert stage:** bị loại — phá invariant
  cô lập converter; stage riêng ngoài sandbox giữ nguyên threat model upload.

## Verification

```bash
# Embedding gate (harness ADR 0005, mở rộng API runtime)
python3 bench/markhand_web/scripts/run_embedding_eval.py --runs 3
python3 bench/markhand_web/scripts/run_retrieval_eval.py

# OCR gate
./target/release/fileconv accuracy <scan-manifest.tsv> bench/REPORT_OCR_VISION.md

# Policy/config
cargo test -p fileconv-server embedding   # runtime_path/policy/normalize-client
cargo test -p fileconv-knowledge --lib identity::tests
```

Denial/negative bắt buộc: provider lỗi → OCR fallback Tesseract giữ job thành
công; embedding lỗi → job pending/backoff, không mất dữ liệu, không trộn
generation; signature mismatch từ chối retrieval; key không xuất hiện trong log.

## Exception lifecycle

| Field | Value |
|---|---|
| Exception | Air-gapped/offline deployment không dùng OpenRouter |
| Owner | operations-owner |
| Scope | OCR = Tesseract-only; embedding = `local-neural` generation riêng |
| Expiry | Không hết hạn — là mode được hỗ trợ chính thức |
| Retest | Mỗi release: e2e ingest không network pass |
