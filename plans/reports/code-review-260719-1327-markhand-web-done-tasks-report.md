# Review chất lượng các task đã hoàn thành — Markhand Web

Ngày review: 2026-07-19
Phạm vi: các task đã đánh dấu Done/merged của kế hoạch
[`plans/markhand-web`](../markhand-web/README.md) — Phase F (12 issues),
Phase 0 (9/10 issues), Phase 1A (10 issues) và phần Phase 1B đã merge
(P1B-F01..F03, PR #213–#217).

Phương pháp: 4 luồng review độc lập đọc toàn bộ code/deliverable, đối chiếu
acceptance criteria trong issue catalog, chạy test/validator khi khả thi
(`cargo test -p fileconv-server`, `cargo test -p fileconv-knowledge
--all-features` 47/47 pass, web test/lint/tsc/api:check pass,
`build-roadmap.py --check` pass, verify checksum bench artifacts).

## Kết luận tổng quát

| Mảng | Verdict |
|---|---|
| Phase 0/F deliverables (docs, bench, gates, CI, roadmap) | **Tốt** — số liệu khớp chéo, checksum verify 100%, trạng thái Done đáng tin |
| Phase 1A — `crates/knowledge` | **Đạt gate thật** — extraction thật, desktop rewire thật, test behavioral mạnh; còn nợ kiến trúc (F1) |
| `web/` connection shell | **Ổn ở trạng thái cuối** — 5 commit fix hội tụ về code đúng; còn thiếu StrictMode + test race |
| Phase 1B — P1B-F01 (runtime/readiness) | **Đạt phần lớn** — config fail-fast, readiness fail-closed; thiếu hoàn toàn logging/tracing |
| Phase 1B — P1B-F03 (schema/migrations) | **Schema tốt, cơ chế enforcement yếu** — integration test hay nhưng **không chạy trong CI** |
| Phase 1B — P1B-F02 (deployment/isolation scaffold) | **CHƯA ĐẠT** — deliverable chính không tồn tại |

Điểm mạnh xuyên suốt: ledger trạng thái trung thực (ghi cả failure:
audio baseline 0/2, citation precision 0.0842, `targetMatch=false`);
issue duy nhất thiếu evidence (P0-05) là issue duy nhất không đánh Done.

## Phát hiện theo mức độ

### Critical

1. **Bộ integration test schema/migration không bao giờ chạy tự động**
   — `crates/server/tests/schema_migrations.rs:48` return sớm (status ok)
   khi thiếu `MARKHAND_TEST_DATABASE_URL`; biến này không được set ở bất kỳ
   đâu trong CI/Makefile/deploy. Đã xác nhận thực nghiệm: `cargo test -p
   fileconv-server` xanh với 8 test DB "pass" trong 0.01s khi không có
   database. Vi phạm trực tiếp yêu cầu "CI test DB rỗng và upgrade path"
   (`phase-1b-single-org-poc.md:274`). Một migration làm hỏng RLS vẫn merge
   được với CI xanh.

### Major

2. **Checksum manifest migration chỉ mang tính trang trí** —
   `crates/server/src/database.rs:206` chỉ so *tên file* với
   `migrations/manifest.json`; giá trị SHA-256 không được so với nội dung
   file. Môi trường fresh sẽ áp SQL bị sửa sau merge mà không phát hiện.
   (Hash hiện tại vẫn khớp file — chỉ thiếu enforcement.)
3. **Seed POC nằm trong chain migration bất biến, chạy vô điều kiện** —
   `migrations/0011_expand_poc_seed.sql` insert org `poc` + user
   `admin@poc.example` + role admin ở mọi lần start server, không gate theo
   profile; đồng thời tồn tại đường seed thứ hai
   (`deploy/scripts/seed-poc-org.sh`) seed user khác → hai nguồn sự thật
   xung đột. DB production sẽ tự nhận org tổng hợp không gỡ sạch được.
4. **P1B-F02 chưa làm** — `deploy/Dockerfile.server`,
   `deploy/Dockerfile.worker`, `deploy/compose.poc.yml` không tồn tại;
   `deploy/dev/compose.yml` chỉ chạy dependencies, không có hardening
   (non-root, read-only FS, cap-drop, no-egress, resource limit) như
   acceptance criteria yêu cầu.
5. **Server không có logging/tracing** — `crates/server/src/http.rs:85`
   vứt bỏ lý do lỗi dependency trước khi cache; không có crate
   `tracing`/`log` trong `Cargo.toml`. Readiness flap trong production sẽ
   không có tín hiệu nào cho operator. Plan P1B.1 yêu cầu OpenTelemetry —
   chưa có; `telemetry.rs` chỉ là contract, runtime không dùng.

### Medium

6. `crates/knowledge` kéo toàn bộ `fileconv-core` (whisper.cpp/cmake,
   pdfium, symphonia…) trong khi chỉ cần `normalize_search_text` +
   `build_corpus` — mọi build/CI của server Phase 1B đều phải compile
   whisper.cpp. Đề xuất tách feature trong core.
7. Tie-ordering không định danh (kế thừa từ desktop, đã chủ đích freeze):
   merge qua `HashSet` + sort coi tie là `Equal`
   (`crates/knowledge/src/desktop/service.rs:366`,
   `src/rank.rs:79`) → hai chunk cùng điểm đổi chỗ giữa các lần chạy;
   fix (`stable_score_order`) đã viết nhưng là dead code. Cần xác nhận
   ticket defect riêng đã được mở như plan yêu cầu (chưa tìm thấy).
8. `web/src/main.tsx:5` thiếu `<StrictMode>` — pattern
   `queueMicrotask`+`disposed` trong `App.tsx` sinh ra chính là để chịu
   double-effect của StrictMode nhưng dev build không bao giờ exercise nó;
   regression cùng loại với chuỗi 5 commit fix sẽ không bị phát hiện.

### Minor (chọn lọc)

9. `crates/server/src/database.rs:97` — lỗi unlock advisory che mất lỗi
   migration gốc; `:176` — `sslmode=require` với self-signed cert sẽ fail
   (nghiêm hơn libpq, fail-closed nhưng dễ bất ngờ khi deploy).
10. `migrations/0006:104` — `UNIQUE (chunk_identity_sha256)` global
    cross-org: nếu công thức identity sau này bỏ salt org/version, tenant
    này dò được sự tồn tại nội dung tenant khác qua constraint-violation.
11. Constants runtime-path embedding bị duplicate giữa
    `crates/knowledge/src/embedding.rs:25` và `crates/core/src/llm.rs:54`,
    không có test equality cross-crate; drift → panic trong Tauri command
    (`app/src-tauri/src/knowledge.rs:68`) hoặc re-embed toàn bộ index.
12. Web test mock dùng chung một `Response` instance
    (`web/src/App.test.tsx:12`) — body single-read, sẽ flake ngay khi có
    fetch thứ hai; thiếu test cho retry/supersession/abort-on-unmount —
    đúng những race mà lịch sử commit đã phải fix.
13. `bench/markhand_web/CORPUS.md` đếm lỗi thời (29 docs/260 queries,
    thực tế 31/268); note gate `G0-RET-BEST-MODEL-GAP` vẫn tả đường GLM
    interim chưa từng đo (evidence thực tế là AITeamVN local CPU);
    `gates.yaml` để `evidence: null` dù summary đã tính gap.
14. `crates/server/src/error.rs:8` — `AppError` dead code, crate dùng
    `Result<_, String>` xuyên suốt; worker binary exit 0 khi không có
    handler → restart loop dưới `restart: unless-stopped`.

## Điểm đã xác minh là tốt

- **Schema multi-org-ready thật**: 26/26 bảng business có `org_id NOT
  NULL`; RLS `ENABLE`+`FORCE` với policy fail-closed (NULL context → 0
  row); immutability enforce bằng trigger DB (cấm DELETE/UPDATE content,
  one-way `draft→published`, at-most-one-current qua partial unique
  index); migration runner có advisory lock + per-file transaction +
  abort khi history checksum lệch.
- **Phase 1A extraction đúng nghĩa**: constants frozen khớp bit-for-bit
  với desktop trước extraction (diff với `53bcb9c~1`), code cũ đã xóa,
  adapter Tauri mỏng, contract 4 IPC command đóng băng bằng JSON fixture;
  không tìm thấy duplication bị cấm ngoài heuristic runtime-path (có
  comment lý do). 47/47 test pass, chất lượng test behavioral cao (binary
  fixture legacy, atomicity, injection, secret-leak).
- **Phase 0 evidence chain nhất quán**: SLA ↔ gates.yaml ↔
  workload-profile ↔ summaries khớp từng số; embedding decision
  (AITeamVN Recall@5 0.9261 ≥ 0.85, bkai 0.7962 FAIL, OpenAI reject pack)
  khớp ADR 0005; 78/78 + 10/10 checksum verify; các validator đều chạy
  trong CI.
- **Web CI đầy đủ**: format/lint/test/api:check/build trên PR, contract
  TS thật sự generate từ `crates/server/openapi/openapi.yaml` (regenerate
  không diff) và được gate chống drift, kể cả khi spec đổi từ phía server.

## Đề xuất ưu tiên (trước khi tiếp tục Phase 1B)

1. Thêm service PostgreSQL vào CI job rust + set
   `MARKHAND_TEST_DATABASE_URL`; đổi test skip thành fail khi thiếu biến
   trong CI (finding 1).
2. Verify SHA-256 manifest với nội dung file trong test/CI + guard thứ tự
   append-only (finding 2).
3. Gate seed 0011 theo profile (dev/POC only) và hợp nhất với
   `seed-poc-org.sh` về một nguồn (finding 3).
4. Làm P1B-F02 thật hoặc đánh lại trạng thái issue cho đúng (finding 4).
5. Thêm `tracing` + log lý do readiness fail (finding 5).
6. Tách feature `fileconv-core` để knowledge/server không build
   whisper.cpp (finding 6).
7. Web: bật StrictMode, sửa mock Response dùng chung, bổ sung test
   retry/abort/supersession (finding 8, 12).
8. Sửa 2 số đếm trong `CORPUS.md`, cập nhật note GLM trong `gates.yaml`
   (finding 13).

## Nguồn

Bốn báo cáo review chi tiết (server, knowledge, web, deliverables) được
tổng hợp trong session Claude Code ngày 2026-07-19; mọi finding nêu trên
đều kèm file:line đã đối chiếu trực tiếp trong working tree tại commit
`63c2f0c`.
