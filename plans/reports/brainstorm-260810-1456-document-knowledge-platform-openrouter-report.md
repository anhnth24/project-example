# Brainstorm Report — Document Knowledge Platform trên Markhand Web (OpenRouter Qwen OCR/embedding)

Ngày lập: 2026-08-10 · Trạng thái: **Draft — chờ product owner duyệt**
Nguồn yêu cầu: spec "Document Knowledge Platform" (85 mục) do product owner cung cấp,
kèm hai quyết định: dùng OpenRouter API key với **Qwen3.7 Flash cho OCR** và
**`qwen/qwen3-embedding-8b` cho embedding**; cải tổ toàn bộ phần local OCR/embedding.
Thiết kế nền trước đó:
[`brainstorm-260713-1656-markhand-web-rag-multi-org-report.md`](brainstorm-260713-1656-markhand-web-rag-multi-org-report.md)
(đã giao qua Phase F→2).

## 1. Bài toán

Spec yêu cầu một Document Knowledge Platform: ingest đa định dạng → parse/OCR →
normalize → structure extraction → Entity → semantic chunk → embedding → hybrid
search → rerank → Q&A + citation, với data model `Resource → Entity → Chunk`,
ba loại text (`raw_text` / `normalized_text` / `embedding_text`), parent-child
retrieval, versioning, incremental embedding, evaluation dataset.

Hai chỉ đạo cụ thể của product owner:

1. **OCR:** Qwen3.7 Flash (OpenRouter) là primary path cho PDF scan; Tesseract giữ làm
   fallback/offline/air-gapped/zero-cost batch. Hai engine phải cùng canonical output.
2. **Embedding:** `qwen/qwen3-embedding-8b` qua OpenRouter thay cho stack local hiện tại.

## 2. Hiện trạng codebase (scout 2026-08-10)

Ba khảo sát độc lập (retrieval/Q&A, OCR, embedding) trên `master d56321d8`.

### 2.1 Phần spec yêu cầu mà hệ thống ĐÃ CÓ (không xây lại)

| Spec | Hiện trạng | Bằng chứng |
|---|---|---|
| Hybrid BM25 + vector + fusion (§40–42) | Có: PG FTS ∥ Qdrant, RRF k=60 + heuristic blend | `crates/server/src/services/retrieval/`, `crates/knowledge/src/rank.rs`; P1B-R01 Done |
| Metadata filter theo org/collection/document (§43) | Có, kèm ACL re-check khi hydrate | `retrieval/hydrate.rs` |
| Citation page/slide/sheet/span + heading (§50) | Có: `SourceAnchor`, `CitationPin` (doc/version hash, quote, span) | `crates/knowledge/src/types.rs`, `services/citation.rs`; P1B-R02 Done |
| Grounded answer, fail-closed extractive (§49) | Có: production ép extractive khi entailment chưa verify | `services/qa/mod.rs` |
| Source viewer trên web (§51) | Có: CitationCard + DocumentPreviewPanel | `web/` QA page; P2 |
| Versioning immutable + as-of/compare/history (§53) | Có: `document_versions`, `is_current`, lineage | migration `0005`; ADR 0002 |
| Checksum/dedup exact (§54) | Có: `content_sha256`, chunk identity SHA-256 | ADR 0006 |
| Async job queue, lease, checkpoint, resume (§69) | Có: job states, `embedding_batches` resume theo ordinal range | `workers/`; P1B-I06 |
| Permission filtering trước LLM (§52) | Có: OrgContext + collection ACL fail-closed | 1C denial suite |
| Retrieval evaluation + gate (§58–60) | Có: golden corpus vi, Recall@5 ≥ 0.85 (`G0-RET-RECALL-AT-5`), MRR/nDCG harness | `bench/markhand_web/`, `run_retrieval_eval.py` |
| Upload/quarantine/sandbox (§68) | Có: quarantine → sandbox converter → artifact | P1B ingest chain |

### 2.2 Gap thật sự so với spec

| Spec | Hiện trạng | Ghi chú |
|---|---|---|
| OCR vision-LLM trong convert pipeline (§17) | **Chưa có.** Convert luôn dùng Tesseract (`image_ocr.rs`); `vision_ocr` (feature `llm`) chỉ là tool opt-in `ocr_hard` (MCP) / hard-OCR ảnh (desktop), không nối vào PDF `needs_ocr` hay server worker | OpenRouter đã là chat preset trong `llm.rs` |
| Embedding cloud cho web (§32–37) | **Bị chặn theo chính sách.** Web pin `AITeamVN/Vietnamese_Embedding` 1024-d local (`local-neural`, Compose `:8088`, ADR 0005); server từ chối `provider-cloud` ngoài `dev` + `MARKHAND_ALLOW_CLOUD_EMBEDDINGS` | Không có preset embedding OpenRouter; chưa có `qwen3-embedding` trong repo |
| Resource → Entity → Chunk (§6–8) | **Chưa có Entity.** Documents/versions ≈ Resource; chunks gắn thẳng version. Gần nhất: `claims` (bảng typed extraction) + document graph MVP | |
| Ba loại text (§11) | **Chưa có.** Chunk chỉ có `body` + `body_text_version=nfc-v1`; payload embedding = `"{heading_path}\n{body}"` | |
| Semantic chunking (§25–26) | **Chưa có.** Chunking cấu trúc heading + 2000 chars (`heading-chunks-2000-v1`) | Đổi chunking = index generation mới (ADR 0006) |
| Parent-child + neighbor expansion (§12, §28, §45–46) | **Chưa có** | |
| Reranker học máy (§44) | **Chưa có** — P1A-06/P1B-R01 chủ động để ngoài scope; hiện chỉ heuristic blend | |
| Incremental re-embed chunk thay đổi (§35, §55) | **Một phần.** Version mới = re-chunk/re-embed toàn bộ version (idempotent, resumable) nhưng chưa diff entity/chunk | |
| Structure extraction generic + domain plugin (§24) | **Chưa có** như một tầng knowledge; PDF "structure" hiện là layout extraction | |
| Observability OCR/token/cost per job (§56–57) | **Chưa có.** `ConversionOutput` không có ocr_engine/duration/page/token/cost | |

Phase 3 hiện tại (P3-01…P3-14) port **document intelligence** desktop (BRD/PRD,
PII, tables, summarize) — **không** trùng với gap trên. Đây là kiến trúc mới, cần
tracking riêng (xem §7).

## 3. Xác minh model OpenRouter (2026-08-10)

| Mục | Kết quả |
|---|---|
| Embeddings API | `POST https://openrouter.ai/api/v1/embeddings`, OpenAI-compatible, có param `dimensions`, `input_type`, `provider.data_collection` |
| `qwen/qwen3-embedding-8b` | Có trên OpenRouter, $0.01/M token, context ~32K; 4096-d native, MRL cho phép giảm chiều (32–4096) |
| Qwen3.7 Flash | Slug `qwen/qwen3.7-flash` (release 2026-07-27, snapshot pin `qwen/qwen3.7-flash-20260727`); vision text+image+video; $0.03/M in, $0.13/M out; 1M context |
| ⚠️ Chất lượng OCR Qwen3.7 Flash | Vision eval độc lập (Roboflow) xếp **hạng chót** OCR transcription (84.1%, #25/25); Alibaba không claim OCR cho tier Flash (tier Plus mới nhắm document parsing). Trải nghiệm nội bộ của product owner tốt trên tài liệu vi — hai nguồn mâu thuẫn ⇒ **bắt buộc đo CER/WER trên corpus scan tiếng Việt của dự án trước khi bật mặc định**, đồng thời benchmark thêm ứng viên đối chứng (vd `qwen/qwen3-vl-32b-instruct` OCR 32 ngôn ngữ, hoặc Qwen3.7 Plus) bằng cùng harness |
| ⚠️ Upstream | Qwen3.7 Flash hiện chỉ một provider phía sau OpenRouter (Alibaba Cloud Int.) — không có failover thứ hai; càng củng cố yêu cầu Tesseract fallback |

## 4. Xung đột chính sách phải xử lý trước khi code

1. **ADR 0005 (Accepted):** web server *không* dùng cloud embedding vì index-time gửi
   toàn bộ chunk text ra ngoài. Chuyển sang OpenRouter = **đảo quyết định này** →
   cần ADR mới ([ADR 0016 draft](../../docs/adr/0016-openrouter-qwen-ocr-embedding.md),
   Status: Proposed) với product-owner approval và benchmark evidence, không sửa ngầm.
2. **Server policy code:** `services/embedding.rs` chỉ cho `provider-cloud` ở
   profile `dev` + `MARKHAND_ALLOW_CLOUD_EMBEDDINGS`. Mở cho prod là thay đổi
   security-triggered (secrets/egress + LLM policy) → bắt buộc security review.
3. **ADR 0006/0011:** đổi model/dimension/runtime_path ⇒ index signature mới ⇒
   expand → backfill → shadow verify → cutover; cấm trộn generation.
4. **Sandbox converter không có network:** vision OCR không thể gọi từ trong sandbox;
   phải thiết kế OCR stage tách riêng (xem §5.1) thay vì đục lỗ sandbox.
5. **OCR cloud = gửi toàn bộ ảnh trang tài liệu ra ngoài** — egress lớn hơn cả
   embedding; ADR 0016 phải nêu rõ data classification nào được phép, và deployment
   air-gapped giữ Tesseract-only.

## 5. Thiết kế đề xuất

### 5.1 Track A — OCR: Qwen3.7 Flash primary, Tesseract fallback

Nguyên tắc spec §17: downstream không phụ thuộc OCR engine; hai engine cùng canonical
output (Markdown theo trang, marker `<!-- Trang N (OCR) -->` như hiện tại; giữ nguyên
phát hiện `needs_ocr` bằng pdf-inspector và render PDFium 300 DPI).

**Core (`fileconv-core`):**

- Thêm engine `vision` vào lựa chọn OCR (`ConverterOptions.ocr_engine`), config qua
  `FILECONV_LLM_*` (OpenRouter đã là preset OpenAI-compatible; `vision_ocr` đã có,
  cần biến thể nhận ảnh in-memory từ PDFium thay vì chỉ file path).
- Semantics **vision-first**: gọi vision cho trang `needs_ocr`; lỗi/timeout/output
  rỗng/nghi hallucination → Tesseract cùng trang; report ghi engine đã dùng per page.
- Desktop/CLI dùng trực tiếp đường này (desktop hiện từ chối hard-OCR PDF — gỡ được
  hạn chế đó luôn).

**Server (`fileconv-server`):**

- **Không mở network cho sandbox.** Convert stage trong sandbox chạy như hiện tại
  (Tesseract cho trang flagged) **và** xuất thêm artifact: danh sách trang `needs_ocr`
  + PNG render của các trang đó.
- Job stage mới `vision_ocr` chạy ở worker tin cậy (ngoài sandbox): gọi OpenRouter
  cho từng trang flagged, thay block trang tương ứng trong Markdown khi thành công;
  thất bại thì giữ nguyên kết quả Tesseract → fallback tự nhiên, đúng spec §17.
- Egress allowlist chỉ `openrouter.ai`; request đặt `provider.data_collection: deny`.
- Secret: `MARKHAND_OCR_API_KEY` (hoặc secret OpenRouter dùng chung) qua env/secret
  mount, redacted Debug, không log.
- Observability (§56–57): job ghi `ocr_engine`, số trang OCR, duration, token
  usage/cost, `requires_review`; không log nội dung trang.

**Gate trước khi bật mặc định:** `fileconv accuracy` (CER/WER) trên corpus scan vi
(bench/*.sh tái tạo) so Qwen3.7 Flash vs Tesseract vs ≥1 VLM đối chứng; chỉ đặt
primary khi thắng Tesseract có số liệu. Model slug pin snapshot
(`qwen3.7-flash-20260727`) trong config deployment, không hardcode trong code.

### 5.2 Track B — Embedding: `qwen/qwen3-embedding-8b` qua OpenRouter

- **Preset:** thêm preset embedding `openrouter`
  (`https://openrouter.ai/api/v1`, `runtime_path=provider-cloud`); cân nhắc thêm
  `openrouter.ai` vào `KNOWN_PROVIDER_DOMAINS` của `embedding_runtime`.
- **Server env:** `MARKHAND_EMBEDDING_BASE_URL=https://openrouter.ai/api/v1`,
  `MODEL=qwen/qwen3-embedding-8b`, `REVISION` pin theo snapshot/PROVIDER routing,
  `RUNTIME_PATH=provider-cloud`. Policy mới (sau ADR 0016): cho phép
  `provider-cloud` ở prod khi flag cho phép cloud embeddings bật **và** deployment
  khai báo data classification tương thích.
- **Normalization:** server hiện fail-closed nếu vector không unit-norm; OpenRouter
  route qua nhiều provider nên không đảm bảo — thêm mode
  `MARKHAND_EMBEDDING_NORMALIZE=client` (normalize rồi verify) thay vì reject.
- **Dimension:** quyết định benchmark: 4096-d native vs 1024-d MRL (param
  `dimensions`). 4096-d tăng ~4× storage/RAM Qdrant so với 1024-d hiện tại — nếu
  1024-d MRL đạt gate thì ưu tiên. Dimension chọn xong là bất biến của generation.
- **Benchmark bắt buộc (điều kiện Ready):** thêm entry API-runtime vào
  `bench/markhand_web/embedding/models.yaml` + harness hỗ trợ HTTP runtime; cùng
  payload/protocol như ADR 0005 (gate `G0-RET-RECALL-AT-5` ≥ 0.85 min-of-3,
  nDCG gap ≤ 0.02). AITeamVN 0.9261 là mốc phải so.
- **Migration (ADR 0011):** generation mới (đổi `embedding_family`, `dimensions`,
  `runtime_path`) → backfill idempotent → shadow verify bằng golden corpus →
  cutover atomic → contract. **Nếu Track C đổi `chunking_version` trong cùng giai
  đoạn thì gộp một generation để tránh rebuild hai lần.**
- **Batching/rate-limit/cost:** giữ batch 64, retry/backoff 429; ghi token
  usage/cost vào job (§56). Ước lượng: corpus 10M chunk × ~400 token ≈ 4B token
  ≈ $40/full rebuild — rẻ, nhưng rate limit mới là ràng buộc chính.
- **Fallback/offline:** giữ profile Compose `embedding-cpu` (AITeamVN) cho
  air-gapped; runtime chọn per deployment, pin per generation. Desktop thêm preset
  OpenRouter embedding (opt-in, cảnh báo egress như GLM hiện tại).

### 5.3 Track C — Knowledge model: Resource/Entity/Chunk + semantic chunking

Theo đúng thứ tự spec §79 (schema → normalizer → structure → entity → chunker →
embedding_text → …), tận dụng tối đa cái đã có:

1. **Canonical schema (expand, không phá):** map `Resource` ≈ `documents` +
   `document_versions` (thêm `resource_type`, `language`, `attributes` JSONB nếu
   thiếu); bảng mới `entities` (`entity_id`, `document_id`, `version_id`,
   `entity_type`, `parent_entity_id`, `title`, `attributes` JSONB, source anchor);
   `chunks` thêm `entity_id`, `raw_text`/`normalized_text`/`embedding_text`
   (giữ identity pin trên raw + version theo ADR 0006).
2. **Markdown normalizer** (§22): conservative — Unicode/whitespace/line-wrap/
   header-footer/page-noise; **bảo vệ identifier** (`89/2026/TT-BTC`, `BR-PAY-022`,
   `POST /payments`, `0,03%`); property test cho invariant "không đổi nghĩa".
3. **Structure parser generic** (heading tree/table/list/formula từ canonical
   Markdown) → **Entity generator** (Document/Section/Table…); domain plugin
   (legal Điều/Khoản/Điểm; BRD Feature/Requirement/BR) là bước sau, deterministic
   trước, không chờ hoàn hảo.
4. **Semantic chunker v2:** ưu tiên entity boundary > heading > paragraph > token
   limit; target 300–800 token, soft 1000, hard 1500; label
   `chunking_version=entity-chunks-v1` ⇒ generation mới (gộp với Track B).
5. **`embedding_text` generator:** title tài liệu + đường dẫn entity + nội dung
   (superset của `"{heading_path}\n{body}"` hiện tại); bảng serialize kèm title +
   column headers per row-group (§29); công thức giữ surrounding context (§30).
6. **Parent-child retrieval + neighbor expansion + context builder** trong
   `crates/knowledge` (dedupe, sort, token budget, citation mapping) — mở rộng
   `ask.rs`/`rank.rs`, không thay kiến trúc retrieval.
7. **Incremental update (§35, §55):** so khớp entity/chunk theo content hash giữa
   version cũ/mới trong cùng generation; chỉ re-embed chunk đổi; đo tỷ lệ reuse.
8. **Reranker (§44):** issue benchmark riêng (baseline RRF+blend vs cross-encoder /
   API reranker — họ Qwen3 có reranker); chỉ thêm nếu số liệu chứng minh đáng cost.
9. **Evaluation (§58–64):** mở rộng golden corpus với ground truth entity-aware;
   benchmark chunking A/B (heading-2000 vs entity-chunks) cùng model — đúng yêu
   cầu spec "không assume semantic chunking luôn tốt hơn".

### 5.4 Kiến trúc sau cải tổ

```text
Upload → quarantine → sandbox convert (parser + Tesseract + render trang needs_ocr)
                            │
                    vision_ocr stage (worker, OpenRouter Qwen3.7 Flash,
                            │           fallback = giữ kết quả Tesseract)
                            ▼
              Markdown canonical → Normalizer → Structure/Entity extractor
                            ▼
        Semantic chunker (entity-chunks-v1) → raw/normalized/embedding_text
                            ▼
        Embedding worker → OpenRouter qwen/qwen3-embedding-8b (provider-cloud)
              │  (air-gapped: AITeamVN local-neural, generation riêng)
              ▼
        Qdrant (generation mới) + PG FTS → hybrid RRF → [reranker nếu thắng bench]
              ▼
        Parent/neighbor expansion → context builder → LLM → answer + citation
```

## 6. Rủi ro chính

| Rủi ro | Giảm thiểu |
|---|---|
| Qwen3.7 Flash OCR kém trên eval độc lập (84.1%, chót bảng) | Gate CER/WER trên corpus vi trước khi primary; benchmark thêm Qwen3-VL/Plus; Tesseract fallback per-page |
| Cloud egress toàn bộ ảnh trang + chunk text | ADR 0016 + security review; `data_collection: deny`; air-gapped profile giữ local |
| OpenRouter routing nhiều provider → vector không ổn định giữa provider | Pin `provider.order` / revision trong config; normalize client-side; shadow verify trước cutover |
| Recall@5 của qwen3-embedding-8b trên corpus vi chưa đo (AITeamVN đang 0.9261) | Gate benchmark là điều kiện Ready; giữ AITeamVN đến khi thắng số liệu |
| 4096-d tăng 4× storage Qdrant | Benchmark MRL 1024-d trước |
| Rate limit/outage OpenRouter làm nghẽn ingest | Retry/backoff, queue đã resumable; embedding lỗi giữ job pending, không mất dữ liệu |
| Rebuild index tốn kém nếu đổi chunking và model hai lần | Gộp một generation duy nhất |

## 7. Phân rã issue đề xuất (draft — chưa tạo catalog/GitHub)

Cần quyết định tracking của owner: gap này **không** thuộc Phase 3 hiện tại
(port intelligence). Đề xuất mở catalog phase mới `plans/markhand-web/backlog/phase-5/`
("Phase 5 — Document Knowledge Platform"), hoặc tách "đợt OpenRouter" vào một
mini-phase riêng nếu muốn giao sớm hơn. Sau khi owner chốt vị trí, dùng skill
`issue-creator` cho từng issue theo đúng format canonical.

**Đợt 1 — cải tổ OCR/embedding (làm được ngay sau ADR 0016 duyệt):**

| ID draft | Outcome | Phụ thuộc |
|---|---|---|
| DKP-01 | ADR 0016 accepted + policy/config cloud embedding & vision OCR | product owner, security review |
| DKP-02 | Benchmark `qwen/qwen3-embedding-8b` (4096 vs 1024 MRL) trên golden corpus, gate ≥ 0.85 | DKP-01, OpenRouter key |
| DKP-03 | Benchmark OCR Qwen3.7 Flash vs Tesseract vs 1 VLM đối chứng (CER/WER corpus scan vi) | DKP-01, OpenRouter key |
| DKP-04 | Core: engine OCR `vision` + fallback per-page, canonical page contract | DKP-03 |
| DKP-05 | Server: stage `vision_ocr` ngoài sandbox + artifact trang render + observability job (engine/token/cost) | DKP-04 |
| DKP-06 | Server: OpenRouter embedding runtime (preset, normalize-client, policy prod) | DKP-01, DKP-02 |
| DKP-07 | Index generation migration: backfill → shadow verify → cutover (gộp chunking mới nếu Đợt 2 sẵn sàng) | DKP-06 |

**Đợt 2 — knowledge model (thứ tự spec §79):**

| ID draft | Outcome |
|---|---|
| DKP-08 | Canonical schema: bảng `entities`, chunks ba-text (expand migration) |
| DKP-09 | Markdown normalizer conservative + property test bảo vệ identifier |
| DKP-10 | Structure parser generic → entity tree (heading/table/list) |
| DKP-11 | Semantic chunker `entity-chunks-v1` + `embedding_text` generator |
| DKP-12 | Parent-child + neighbor expansion + context builder |
| DKP-13 | Incremental re-embed theo entity/chunk diff giữa version |
| DKP-14 | Eval mở rộng: chunking A/B, entity-aware ground truth |
| DKP-15 | Reranker benchmark (chỉ ship nếu thắng số liệu) |
| DKP-16 | Domain plugin: legal (Chương/Điều/Khoản/Điểm) + BRD/SRS entities |

Ngoài scope (đúng spec §2, §77): source code graph, GraphRAG, multi-agent,
Neo4j, multimodal embedding, business-code linking.

## 8. Next steps

1. Product owner review report này + [ADR 0016 draft](../../docs/adr/0016-openrouter-qwen-ocr-embedding.md);
   chốt tracking location (Phase 5 mới hay mini-phase OpenRouter).
2. Cung cấp OpenRouter API key qua secret (Cloud Agents → Secrets / deployment env
   `MARKHAND_EMBEDDING_API_KEY`, `MARKHAND_OCR_API_KEY`) — **không commit**.
3. Chạy DKP-02/DKP-03 (hai benchmark) — đây là evidence bắt buộc để ADR 0016
   chuyển Accepted và các issue implementation chuyển `Ready`.
4. Dùng `issue-creator` tạo catalog entry cho từng issue sau khi owner duyệt draft.

## Unresolved questions

- Data classification nào được phép đi qua OpenRouter (toàn bộ tenant hay chỉ org
  opt-in)? Ảnh hưởng thiết kế policy per-org của DKP-06.
- Dimension cuối cùng (4096 vs 1024 MRL) — chờ DKP-02.
- Qwen3.7 Flash có qua nổi gate CER/WER tiếng Việt không, hay phải nâng tier
  (Plus/Qwen3-VL) — chờ DKP-03; kéo theo cost model.
- Desktop có theo OpenRouter embedding mặc định không, hay giữ local-hash/Ollama
  (đề xuất: giữ nguyên, OpenRouter chỉ là preset opt-in).
