# Phase 2Q issues — Nâng cấp chất lượng Q&A

Parent plan: [`../../../phase-2q-qa-quality.md`](../../../phase-2q-qa-quality.md)

<!-- roadmap-default-status: backlog -->

Mọi issue ở **Backlog**. Thiết kế chi tiết từng hạng mục nằm ở plan nguồn
[`../../../../260817-qa-quality-upgrade.md`](../../../../260817-qa-quality-upgrade.md)
(đã red-team 2026-08-17); catalog này là authority về scope/trạng thái.
P0.0/P0.0b/P0.2 của plan nguồn đã giao trước khi lập phase — không có issue
hồi tố.

## Dependency

```text
P2Q-01 (reindex + re-baseline) ── độc lập, làm sớm
P2Q-02, P2Q-03 ── độc lập
P2Q-04 ── gate bench NLI đầu issue quyết đi tiếp/hạ warning
P2Q-05 → P2Q-06 → P2Q-07
P2Q-08 ── sau khi P2Q-01 re-baseline; đụng fileconv-core (CLI/desktop/server)
P2Q-09 ── độc lập; hữu ích sau P2Q-03
```

## P2Q-01 — Title tài liệu vào embedding input (index generation mới)

- **Plan/files:** `workers/embedding.rs` `canonical_input(title, heading_path,
  body)`; đổi `input_sha256` ⇒ index signature mới ⇒ **generation mới** qua
  đường `index_metadata` đã có, không hồi tố generation active; code +
  `approved_signature`/config deploy **cùng nhịp** (lệch là worker từ chối job
  — ghi checklist deploy vào PR).
- **Depends:** không; sau reindex phải re-baseline cả hai eval trước khi đánh
  giá PR kế tiếp. **Acceptance/tests:** eval format tăng nhóm cross-doc/XLSX,
  eval version không giảm >1 điểm median (≥2 run); test worker từ chối job khi
  signature lệch.
- **Security/migration:** đổi index signature/reindex — review bắt buộc; không
  đổi schema PostgreSQL. **Out:** đổi embedding provider/model/dimension.

## P2Q-02 — Tầng cảnh báo xung đột số liệu (warning-only)

- **Plan/files:** `services/qa/grounding.rs`: câu có số nhưng không trích được
  value theo pattern hiện tại → so số theo **kiểu giá trị** (% với %, tiền với
  tiền, MWh với MWh) giữa câu và passage; lệch → warning, chưa prune. Loại
  trước khi so: số VN `1.234,56`, "Điều 5", số hiệu văn bản, ngày tháng.
  Warning kèm entry `WARNING_TRANSLATIONS` + test web.
- **Depends:** P0.2 (đã giao). **Acceptance/tests:** unit case groundguard
  300%↔30%, 4.8↔5 + case số Việt; không prune câu nào vòng này; nâng thành
  gate chỉ ở PR sau khi đo FP = 0 trên 2 bộ eval.
- **Security:** grounding policy — review bắt buộc. **Out:** prune/gate ngay
  vòng này; so số trần không phân kiểu.

## P2Q-03 — Completeness check sau OCR trước khi index

- **Plan/files:** worker convert / `services/vision_ocr.rs` /
  `services/ocr_normalize.rs`: so tín hiệu mật độ text của ảnh render với độ
  dài/độ phủ output OCR; nghi thiếu → retry giới hạn rồi flag document +
  warning, không index thầm bản cụt.
- **Depends:** không. **Acceptance/tests:** case thật
  `quyet-dinh-thanh-lap-to-ai.png` (OCR trả 112 ký tự tiêu đề, mất Điều 1–3,
  tái lập ổn định) bị bắt; 4 ảnh đủ chữ của corpus đợt 2 không false-positive;
  flag/warning hiển thị được ở web.
- **Security:** không log nội dung tài liệu; đụng OCR/LLM path — review bắt
  buộc. **Out:** đổi model/provider OCR; auto re-OCR không giới hạn.

## P2Q-04 — Bench gate + structured entailment NLI local

- **Plan/files:** Gate đầu issue: bench NLI đa ngữ trên tiếng Việt
  (ứng viên mDeBERTa-v3-xnli, XLM-R NLI; license check theo quy trình
  PhoWhisper) bằng bộ câu grounded/ungrounded gán nhãn từ eval log — đo cả
  precision lẫn latency thật (ước 100–400ms/cặp CPU). Đạt → chạy sau
  claim-check lexical cho câu đã pass, entailment ≥ ngưỡng mới grounded, có
  timeout budget cho cả answer (quá hạn giữ hành vi dev-gate, không silent-pass,
  không treo stream); kém → hạ vai trò xuống warning-only.
- **Depends:** gate bench pass mới triển khai; fail thì đóng issue với evidence
  bench. **Acceptance/tests:** báo cáo bench precision/latency; khi bật: đường
  warning "structured entailment is NOT available" được gỡ; không có model →
  hành vi hiện tại nguyên vẹn.
- **Security:** model binary không commit, identifier không vào code
  (CLAUDE.md); grounding/LLM policy + native binary — review bắt buộc.
  **Out:** gọi API ngoài để entailment; cross-encoder rerank.

## P2Q-05 — Tự nhận diện intent version từ câu hỏi (rule-based)

- **Plan/files:** tầng rule deterministic (không LLM): pattern
  "phiên bản/bản X", "thay đổi gì/khác gì/vì sao đổi", "so với", "trước
  ngày/tại thời điểm" → suy mode `compare/as_of/history` + tham số khi xác
  định được document qua anchor token; UI hiển thị mode đã nhận diện **nổi
  bật** + revert 1 click; log tỷ lệ override. Không đổi contract `AskRequest`
  — client set field sẵn có.
- **Depends:** hạ tầng version modes hiện có (P1B/P2-10). **Acceptance/tests:**
  ma trận rule unit test; câu version-sensitive gõ ở mode current được route;
  không nhận diện được → giữ `current`; UI test revert.
- **Security:** N/A — không đổi persisted contract; rủi ro đọc nhầm version cũ
  được chặn bằng UI revert bắt buộc. **Out:** LLM intent classifier.

## P2Q-06 — Bộ câu hỏi implicit-change cho eval version

- **Plan/files:** mở rộng harness eval version: nhóm câu về thay đổi KHÔNG
  ghi trong changelog corpus; đo và commit baseline (chắc chắn thấp — đó là
  baseline) vào `bench/markhand_web/reports/` trước khi làm P2Q-07.
- **Depends:** P2Q-05 (route intent để chạy đúng mode). **Acceptance/tests:**
  bộ câu + số baseline nằm trong report; là tiêu chí nghiệm thu của P2Q-07.
- **Security:** corpus tự sinh, không dữ liệu khách hàng. **Out:** change
  index (P2Q-07).

## P2Q-07 — Bảng `version_changes` và retrieval theo intent change

- **Plan/files:** migration expand-only bảng `version_changes`: mỗi bản ghi =
  một section thay đổi giữa version N và version cha, lưu **tham chiếu 2 span
  thật** (chunk_id/span version cũ + mới), heading, loại thay đổi, diff tóm
  tắt deterministic; sinh lúc publish version N>1 (diff canonical markdown đã
  normalize, bỏ whitespace, ngưỡng độ dài). Retrieval chỉ tra khi intent là
  change; hydrate 2 chunk thật làm passage; citation trỏ 2 chunk thật —
  `verify_dual_spans` nguyên vẹn. Không sinh chunk mới, không vào index vector
  bước đầu.
- **Depends:** P2Q-05, P2Q-06. **Acceptance/tests:** điểm bộ P2Q-06 tăng;
  mode current không ô nhiễm (`version_changes` không nằm trong retrieval
  thường); diff không thấy thay đổi → fail-closed, không citation ảo.
- **Security/migration:** SQL/RLS/migration — review bắt buộc; RLS org-scope
  như các bảng khác. **Out:** embed diff text vào vector (cân nhắc PR sau);
  GraphRAG.

## P2Q-08 — Chunking theo loại tài liệu (fileconv-core, đa consumer)

- **Plan/files:** `crates/core/src/chunk.rs` biến thể theo `FormatKind`: văn
  bản pháp quy khóa ranh giới chunk theo Điều/Khoản (khớp
  `promote_legal_headings` phía server); bảng dài lặp header mỗi chunk con;
  PPTX mỗi slide một chunk. **Consumers:** CLI `fileconv`, desktop Markhand,
  server worker index — thay đổi core-only nhưng phải đo cả ba.
- **Depends:** làm sau khi P2Q-01 re-baseline (tránh nhiễu chồng).
  **Acceptance/tests:** `fileconv accuracy`/`speed` không thoái hóa
  (quy tắc CLAUDE.md); eval server tăng nhóm bảng/pháp quy; desktop CI xanh.
- **Security/migration:** chunking version nằm trong index signature pin
  (invariant 7) ⇒ áp lên server cần generation mới + re-baseline như P2Q-01;
  review bắt buộc. **Out:** đổi API `chunk.rs` public contract ngoài mức cần;
  sửa chunk tay.

## P2Q-09 — UI xem chunk đã parse (read-only)

- **Plan/files:** API list chunk theo document version (ACL như preview,
  org-scope + RLS hiện có) + web UI: hiện `heading_path` + body từng chunk,
  đánh dấu chunk nghi hỏng OCR (heading rỗng, mật độ ký tự bất thường).
- **Depends:** không bắt buộc; flag từ P2Q-03 làm giàu hiển thị.
  **Acceptance/tests:** xem đúng chunk theo version; denial test cross-org/
  cross-user theo khuôn ACL hiện có; không lộ chunk ngoài quyền.
- **Security:** surface đọc mới trên chunks — ACL/RLS review bắt buộc;
  read-only. **Out:** sửa chunk tại chỗ (re-publish version — thiết kế riêng
  giai đoạn sau).
