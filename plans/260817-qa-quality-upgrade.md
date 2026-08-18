# Plan nâng cấp chất lượng Q&A (học từ RAGFlow, VersionRAG, groundguard/Warrant, Onyx, Kotaemon)

Ngày lập: 2026-08-17. Đã qua red-team (ak:predict, verdict CAUTION) ngày
2026-08-17 — bản này đã sửa theo toàn bộ khuyến nghị: P2.2 thiết kế lại để
không phá mô hình citation integrity, P0.2 đảo thứ tự đề-xuất-trước-validate-sau,
P0.3 hạ xuống warning-only, thêm P0.0, eval câu hỏi change lên trước change
index, siết tiêu chí đo.

Nguồn: deep-dive kiến trúc + code của các hệ document-to-QA mã nguồn mở, đối
chiếu từng điểm với code Markhand hiện tại.

## 0. Hiện trạng và baseline đo được

Điểm eval hiện tại (harness `mh-eval-version.py` / `mh-eval-format.py`, corpus IT
versioned + đa định dạng, 2026-08-17):

| Bộ đo | Điểm | Ghi chú |
| --- | --- | --- |
| Version-aware (32 câu) | 98.8 | reasoning mặc định; 90.6 khi `MARKHAND_CHAT_REASONING=off` |
| Đa định dạng tổng | 91.8 | DOCX 91.7 · XLSX 94.3 · Image 85.0 · Trap 100 |

Những gì Markhand ĐÃ có (không làm lại):

- Pipeline chuẩn ngành: convert → `ocr_normalize` → chunk theo heading →
  Postgres FTS (accent-fold) + Qdrant vector → merge rerank + anchor-aware
  diversity → LLM + citation `[CITE-]` → grounding fail-closed.
- Hạ tầng version **mạnh hơn đa số repo ngoài**: `document_versions` bất biến,
  `effective_from/to`, con trỏ `is_current` DB-enforced
  (`migrations/0005`), `VersionVisibility::{Current, VersionIds}`,
  `resolve_as_of_version_ids`, 4 mode ask `current/as_of/compare/history` đã
  nối tới tận UI (`ChatPanel.tsx`).
- Grounding lexical: `passage_supports_sentence` (giá trị số/mã bắt buộc khớp),
  `negation_contradicts`, prune từng câu, citation retry 1 lần.
- **Ràng buộc kiến trúc phải tôn trọng** (red-team xác nhận từ `citation.rs`):
  mọi chunk được cite phải có `span_start/span_end` xác minh byte-exact vào
  canonical markdown (`verify_dual_spans`, `chunk_identity`, `stable_anchor`).
  Không hạng mục nào được sinh "chunk tổng hợp" không có span thật.

## 1. Khoảng trống so với các repo tham chiếu

| # | Khoảng trống | Repo tham chiếu / bằng chứng |
| --- | --- | --- |
| G0 | Lỗi transient "Embedding timed out" lúc ask làm rớt nguyên câu hỏi (Image 85.0 trong eval 2026-08-17 có 1 câu rớt vì lỗi này) | — (phát hiện nội bộ, red-team bổ sung) |
| G1 | Embedding input chỉ có `heading_path + body` — thiếu tiêu đề tài liệu (`workers/embedding.rs` `canonical_input`) | RAGFlow embed title+text; Onyx thêm contextual metadata khi chunk. Câu hỏi cross-doc ("PM2", "Thông tư 36") hiện chỉ được cứu ở tầng rerank (anchor diversity), muộn hơn cần thiết |
| G2 | Structured entailment chưa có → dev-gate cảnh báo "unverified" | Warrant/anchor-guard: NLI cross-encoder **local** kiểm entailment từng câu, không cần API ngoài |
| G3 | Kiểm claim chưa có tầng bắt xung đột số liệu riêng (30% vs 300%, 4.8 vs 5) khi câu không chứa giá trị trích được | groundguard tier 2.5 "numerical detector" chạy trước LLM |
| G4 | Citation retry tốn thêm 1 lượt LLM; retry fail → rớt extractive dù nội dung đúng | RAGFlow `insert_citations` (rag/nlp/search.py:261): gắn citation hậu kỳ bằng hybrid similarity (token 0.1 + vector 0.9, ngưỡng 0.63 giảm dần ×0.8), không tốn LLM |
| G5 | Người dùng phải TỰ chọn mode as_of/compare/history; câu hỏi tự nhiên "PM2 đổi gì ở bản 2.0?" gõ ở mode current không được route | VersionRAG: intent classification (content/version/change) rồi route — 90% vs 58% naive trên câu hỏi version-sensitive |
| G6 | Chưa trả lời được thay đổi NGẦM giữa version — chỉ trả lời được thay đổi corpus tự ghi trong "Lịch sử thay đổi" | VersionRAG: change index (explicit từ changelog + implicit từ diff) nâng implicit-change accuracy 0–10% → 60% |
| G7 | Một chiến lược chunk duy nhất (heading) cho mọi loại tài liệu | RAGFlow: template per-type (laws theo Điều/Khoản, table giữ header từng chunk, presentation per slide) |
| G8 | Không xem/sửa được chunk đã parse — OCR hỏng chỉ phát hiện khi Q&A sai | RAGFlow: UI xem + sửa chunk, "quality in, quality out" |

Đã cân nhắc và KHÔNG đưa vào plan: cross-encoder rerank model (hybrid weights
hiện đủ ở quy mô này, thêm model = thêm latency); GraphRAG/knowledge graph
(VersionRAG chứng minh graph tối giản theo domain thắng GraphRAG tổng quát với
1/16 chi phí index); question decomposition đa hop (anchor diversity đang xử lý
được lớp câu hỏi hiện tại — chỉ mở lại nếu eval xuất hiện câu multi-hop fail).

## 2. Các phase

Nguyên tắc: mỗi hạng mục một PR riêng (quy tắc repo). Tiêu chí đo chung ở mục 3.

### Phase 0 — Quick wins (retrieval + citation, không đổi schema)

**P0.0 Retry lỗi embedding transient lúc ask** (sửa G0 — rẻ nhất toàn plan)
- Đường ask/ask_stream: lỗi "Embedding timed out"/transient từ provider →
  retry đúng 1 lần với backoff ngắn trước khi trả lỗi cho người dùng.
- Không retry lỗi 4xx/quota (không phải transient). Warning khi phải retry.
- Kỳ vọng: xóa nhóm lỗi đã làm Image mất ~10 điểm trong một lần chạy eval.
- ✅ DONE 2026-08-17: `embed_query_with_retry` trong
  `crates/server/src/services/retrieval/mod.rs` (retry 1 lần, backoff 400ms,
  chỉ retry timeout + `EmbeddingError::Http`); warning
  "Embedding recovered after one retry" đã có bản dịch tiếng Việt + test
  (`warningPresentation.test.ts`). 37 test retrieval pass, fmt sạch.

**P0.0b Split file-token trong FTS tsv** (phát hiện eval format v2, 2026-08-18)
- Triệu chứng: hỏi "công văn 1502" không bao giờ ra tài liệu
  `CÔNG VĂN SỐ 1502/CV-CNTT` — parser `simple` của PostgreSQL coi
  "1502/CV-CNTT" (và ngày "27/08/2026") là MỘT token kiểu `file`, token
  truy vấn "1502" không match. Ảnh hưởng mọi số hiệu văn bản hành chính VN.
- ✅ DONE 2026-08-18: migration `0037_expand_chunks_split_file_tokens_tsv.sql`
  — tsv = hợp của 2 folding (nguyên khối + thay `/` bằng space); backfill cần
  `SET LOCAL row_security = off` vì `chunks` bật FORCE RLS (bài học: backfill
  0016 trước đây cũng bị RLS nuốt im lặng). Verify: query "công văn 1502",
  "quyết định 88/QĐ-CNTT tổ AI" đều trả đúng tài liệu ở hit #1.
- Known issue còn lại (chưa sửa): câu hỏi DÀI vẫn có thể trượt FTS vì
  `plainto_tsquery` AND toàn bộ token (thiếu 1 từ như "phải" là fail) —
  vector leg phải gánh; P0.1 (title vào embedding) sẽ giúp thêm.

**Known issue OCR (2026-08-18, chưa có hạng mục)**: vision model dừng sớm
tái lập ổn định với `quyet-dinh-thanh-lap-to-ai.png` (chỉ trả 112 ký tự
tiêu đề, mất Điều 1–3; ảnh render đầy đủ, `finish_reason` không phải
"length", re-OCR 2 lần cùng kết quả). 4 ảnh còn lại của đợt 2 OCR đủ.
Cần hạng mục riêng: completeness check sau OCR (ví dụ so mật độ text
render vs độ dài output) trước khi index.

**P0.1 Thêm tiêu đề tài liệu vào embedding input** (sửa G1)
- `ApprovedEmbeddingRuntime::canonical_input(title, heading_path, body)`.
- LƯU Ý 1: đổi canonical input ⇒ đổi `input_sha256` và index signature ⇒ phải
  là **generation index mới** + reindex (đi đúng đường `index_metadata`
  generation đã có). Không hồi tố lên generation đang active.
- LƯU Ý 2 (red-team): code + `approved_signature`/config phải cập nhật trong
  **cùng một lượt deploy**, lệch nhịp là worker từ chối job embedding
  ("index signature mismatch"). Ghi checklist deploy vào PR.
- Sau khi reindex: **re-baseline cả hai eval** trước khi đánh giá bất kỳ PR
  nào sau đó.
- Đo: eval format (kỳ vọng XLSX/cross-doc tăng), eval version không giảm.

**P0.2 Gắn citation hậu kỳ — đề xuất trước, validate sau** (sửa G4)
- Thứ tự bắt buộc (red-team: claim-check hiện chạy THEO pin đã cite, nên
  không tồn tại trạng thái "pass claim-check nhưng thiếu cite"):
  1. Câu trong draft thiếu `[CITE-]` → tìm passage ứng viên có token-overlap
     cao nhất với câu (trên `snippet/body` của `hybrid` hits sẵn có — không
     gọi embed lại) vượt ngưỡng;
  2. Gắn thử `[CITE-xxxx]` của ứng viên vào câu;
  3. Chạy **nguyên bộ** validation hiện có (`validate_answer_citations` +
     claim-check + negation) với pin đó;
  4. Pass → giữ câu với citation đã gắn + warning "citation auto-attached";
     fail → prune như hiện tại. Fail-closed nguyên vẹn.
- Vị trí so với citation retry: thử auto-attach TRƯỚC; chỉ retry LLM khi
  auto-attach không cứu được câu nào (tiết kiệm 1 call LLM/câu lỗi).
- "Cứu được 3/22 câu DOCX" là **giả thuyết cần đo**, không phải kỳ vọng cam kết.
- Warning mới phải kèm entry `WARNING_TRANSLATIONS` + test (quy ước từ nay
  áp cho mọi PR có warning mới).
- ✅ DONE 2026-08-18: `propose_citation_for_sentence` (grounding.rs — overlap
  ≥2 token và ≥35% token câu) + `attach_citations_to_uncited_lines` (mod.rs —
  gắn thử rồi chạy NGUYÊN BỘ `validate_answer_citations`, fail → None).
  Hook cả 2 chỗ theo đúng thứ tự plan: (1) trước prune trong
  `resolve_llm_answer`; (2) trước citation-retry trong `ask()` +
  `ask_stream` (cứu được thì khỏi tốn lượt LLM nhắc lại). Warning
  "Auto-attached citations to N sentence(s)" có bản dịch + test web.
  50 unit test qa pass; verify live: câu "Lịch chốt số liệu đối soát…"
  được auto-attach 2 câu, không cần retry.
- 📊 Đo 2026-08-18 (eval format v2, 51 câu + 4 trap): run3 (sau P0.2 + P0.0b)
  **92.9** vs run1 baseline 85.7 (run2 79.4 nhiễu 429 OpenRouter giờ cao
  điểm — 2 câu 0 hits vì embedding 429 cả 2 lượt). DOCX 96.9 / XLSX 97.5 /
  Image 82.0 / trap 4/4. Auto-attach kích hoạt 15/51 câu; fallback_extractive
  giảm 8 → 1. Điểm image còn lại kẹt ở 2 câu quyet-dinh-to-ai (OCR cụt —
  known issue trên) + 1 câu hỏi dài trượt FTS AND-semantics (chờ P0.1).
  Eval version (IT corpus) 89.1 = đúng baseline, không tụt.

**P0.3 Tầng bắt xung đột số liệu — warning-only trước** (sửa G3)
- `grounding.rs`: khi câu có số nhưng KHÔNG trích được value theo pattern hiện
  tại, so số theo **kiểu giá trị** (% với %, tiền với tiền, MWh với MWh —
  không so số trần) giữa câu và passage; lệch → **warning**, chưa prune.
- Red-team: số Việt Nam (1.234,56), "Điều 5", số hiệu văn bản, ngày tháng
  không phải claim giá trị — phải loại trước khi so.
- Chỉ nâng thành gate (prune) ở PR sau, khi đo được FP rate qua eval +
  đối chiếu warning log < ngưỡng thống nhất (đề xuất: 0 FP trên 2 bộ eval).
- Unit test bằng case groundguard nêu: 300%↔30%, 4.8↔5, cộng case số Việt.

### Phase 1 — Structured entailment local (mở khóa dev-gate, sửa G2)

- **Gate vào phase (làm trước, fail thì dừng cả phase)**: bench NLI đa ngữ
  trên tiếng Việt bằng bộ câu grounded/ungrounded gán nhãn từ eval log
  (ứng viên: mDeBERTa-v3-base-xnli, XLM-R NLI — XNLI có tiếng Việt; license
  kiểm tra như quy trình PhoWhisper). Đo cả precision lẫn **latency thật**:
  ước lượng 100–400ms/cặp CPU là hiện thực, không phải ≤50ms.
- Model binary không commit vào repo, không ghi identifier vào code (quy tắc
  CLAUDE.md); phân phối qua cơ chế approved-runtime đã có
  (`UnapprovedEmbeddingRuntime` pattern) — đây là thay đổi grounding/LLM
  policy ⇒ review bắt buộc theo `repository-delivery.mdc`.
- Vị trí: sau claim-check lexical, chỉ chạy cho câu ĐÃ pass lexical (giảm số
  cặp phải chấm); entailment ≥ ngưỡng → grounded; ngược lại prune.
- Ngân sách thời gian có cắt (timeout budget cho cả answer): quá hạn →
  giữ nguyên hành vi dev-gate hiện tại, không silent-pass, không treo stream.
- Precision kém trên tiếng Việt → hạ vai trò xuống tín hiệu warning (không
  gate), giữ dev-gate warning như cũ.
- Khi runtime có model và gate bật: gỡ đường warning "structured entailment
  is NOT available"; không có model → hành vi hiện tại.

### Phase 2 — Version intelligence (VersionRAG-lite, sửa G5+G6)

**P2.1 Tự nhận diện intent version từ câu hỏi**
- Tầng 1 rule-based (không LLM, deterministic): pattern "phiên bản/bản X",
  "thay đổi gì / khác gì / vì sao đổi", "so với", "trước ngày / tại thời
  điểm", số hiệu thông tư + "cũ/mới" → suy ra mode `compare/as_of/history`
  và tham số (version number, mốc thời gian) khi xác định được document
  qua anchor token.
- UI: hiển thị mode đã tự nhận diện **nổi bật** + revert 1 click (red-team:
  auto-route nhầm khi câu nhắc "trước ngày" tình cờ sẽ trả lời từ version cũ
  mà user không nhận ra). Log tỷ lệ người dùng override để đo chất lượng rule.
- Không tự route được thì giữ nguyên `current` (an toàn, hành vi cũ).
- Không đổi contract `AskRequest` — client tự set các field sẵn có.

**P2.2 Bộ câu hỏi implicit-change cho eval (làm TRƯỚC change index)**
- Mở rộng `mh-eval-version.py`: nhóm câu hỏi về thay đổi KHÔNG ghi trong
  changelog của corpus (đo được hiện tại chắc chắn thấp — đó chính là
  baseline). Không có bộ này thì P2.3 không có tiêu chí nghiệm thu.
  (VersionRAG cũng xây benchmark trước khi xây hệ.)

**P2.3 Index thay đổi giữa version — bảng riêng, citation 2 span thật**
- **Thiết kế lại theo red-team** (bản cũ sinh "change chunk" tổng hợp — phá
  mô hình citation integrity vì không có byte-span trong canonical markdown):
  - Bảng mới `version_changes` (migration expand-only): mỗi bản ghi = một
    section thay đổi giữa version N và version cha, lưu **tham chiếu 2 span
    thật**: (chunk_id/span ở version cũ) + (chunk_id/span ở version mới),
    heading, loại thay đổi (thêm/sửa/xóa), text diff tóm tắt deterministic.
  - Sinh lúc publish version N>1: diff canonical markdown ĐÃ normalize theo
    section heading; bỏ khác biệt whitespace; ngưỡng tối thiểu độ dài.
  - Retrieval: intent "change" (từ P2.1) → tra `version_changes` của document
    liên quan, hydrate 2 chunk thật ở 2 version làm passage; citation của câu
    trả lời trỏ vào 2 chunk thật đó — khớp mô hình pin compare-mode đã có,
    `verify_dual_spans` hoạt động bình thường.
  - Không sinh chunk mới, không đưa gì vào index vector ở bước đầu (FTS/tra
    trực tiếp theo document đủ dùng vì đã biết document từ intent); cân nhắc
    embed diff text ở PR sau nếu eval cho thấy cần.
- Mode current không bị ô nhiễm: `version_changes` chỉ được tra khi intent
  là change — không nằm trong đường retrieval thường.
- Đo bằng bộ câu hỏi P2.2.

### Phase 3 — Chunking theo loại tài liệu (sửa G7)

- `chunk.rs`: thêm biến thể theo `FormatKind`/cấu trúc phát hiện được:
  - Văn bản pháp quy: ranh giới chunk khóa theo Điều/Khoản (đã có
    `promote_legal_headings` phía server — đảm bảo chunker không cắt giữa Điều).
  - Bảng (XLSX/CSV/bảng markdown): lặp lại hàng header trong mỗi chunk con khi
    bảng dài phải cắt.
  - PPTX: mỗi slide một chunk.
- Đây là code dùng chung CLI/desktop/server (`fileconv-core`) — đo lại bằng
  `fileconv accuracy`/`speed` theo quy tắc CLAUDE.md, ngoài eval server.

### Phase 4 — Vận hành chất lượng (sửa G8, làm sau cùng)

- UI "xem chunk đã parse" cho một document version (read-only trước): lộ
  heading_path + body từng chunk, đánh dấu chunk `heading_path` rỗng hoặc mật
  độ ký tự lạ cao (dấu hiệu OCR hỏng). Sửa tay (RAGFlow-style) để giai đoạn
  sau — đụng immutability của chunks nên cần thiết kế riêng (re-publish
  version mới thay vì sửa tại chỗ).

## 3. Thứ tự, phụ thuộc, tiêu chí giữ/loại

```
P0.0        ──►  độc lập, làm ĐẦU TIÊN (rẻ nhất, xóa noise eval)
P0.2, P0.3  ──►  Phase 1 (cùng vùng grounding — làm P0 trước để đo sạch)
P0.1        ──►  độc lập (cần reindex + re-baseline — làm sớm)
Phase 1     ──►  gate bench NLI đạt mới vào phase
P2.1        ──►  P2.2 (eval) ──► P2.3 (change index)
Phase 3, 4  ──►  độc lập, sau khi 0–2 ổn định
```

Tiêu chí mỗi PR:
- (a) Eval quyết định giữ/loại chạy **≥2 lần lấy median** (LLM nondeterministic,
  1 lần chạy có noise > 1 điểm): median không giảm quá 1 điểm so với baseline
  gần nhất, hạng mục nhắm tới phải tăng. Sau mỗi lần đổi index generation
  (P0.1) phải re-baseline trước khi đánh giá PR kế tiếp.
- (b) Test QA/retrieval/web pass.
- (c) `cargo fmt --all -- --check`, `cargo metadata --locked`, dependency
  policy script pass.
- (d) Hạng mục đụng grounding/LLM policy/contract/migration đi qua review
  bắt buộc theo `repository-delivery.mdc`.
- (e) PR có warning mới phải kèm entry `WARNING_TRANSLATIONS` + test.

## 4. Rủi ro chính (sau red-team)

- **P0.1 reindex**: bắt buộc generation mới, corpus phải re-embed — chi phí
  provider; deploy code + signature cùng nhịp (worker từ chối job nếu lệch);
  làm trên dev, re-baseline, rồi mới nói chuyện production.
- **Phase 1 NLI tiếng Việt**: precision chưa chắc; latency thật 100–400ms/cặp
  — gate bench đầu phase quyết định đi tiếp/dừng; kém thì hạ xuống warning.
- **P2.1 auto-route nhầm**: UI phải hiển thị mode nhận diện + revert 1 click;
  log override rate.
- **P2.3 diff nhiễu**: chỉ diff canonical markdown đã normalize, bỏ whitespace,
  ngưỡng độ dài; mọi citation đều trỏ span thật nên không có đường "citation
  ảo" kể cả khi diff sai — tệ nhất là không tìm thấy thay đổi (fail-closed).
- **Phase 3 đụng converter dùng chung**: ảnh hưởng CLI/desktop — bench
  `fileconv accuracy` bắt buộc trước khi merge (quy tắc CLAUDE.md).
