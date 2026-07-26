# Phase 1B R02→R06 close runbook + O05 real-embedding plan + Profile B boundary

Date: 2026-07-26
Base commit: `3048839` (Pass the Phase 1B O-chain end to end on live infrastructure, #309)
Nguồn: `plans/markhand-web/backlog/phase-1b/issues/README.md` (issue catalog),
`plans/reports/gate-run-260726-2050-markhand-web-phase1b-o-chain-pass.md` (O-chain pass),
`bench/markhand_web/gates.yaml`, `docs/markhand-web-risk-register.md`, `.github/workflows/ci.yml`.

## 1. Runbook đóng R02→R06

Thứ tự phụ thuộc theo catalog (`README.md` dòng ~524-526):
`... R01/R02/R03 → R04/R05/R06 → O04/O03 → O05`. Trong nhóm R04/R05/R06,
`R06` khai báo `Depends: R04/R05/F05` nên bản thân R06 chỉ đóng được sau R04 và
R05. Bên dưới trình bày theo thứ tự **R04 → R02 → R03 → R05 → R06** (R04 không
phụ thuộc R02/R03 trong `Depends` của nó — chỉ F04/F05/I01/I03/I07/R02 — nhưng vì
R02 nằm trong `Depends` của R04, thứ tự thực đóng vẫn phải chờ R02 trước R04; xem
ghi chú "no existing command found" bên dưới cho phần tôi không thể xác nhận thêm
về sequencing ngoài catalog).

[Unverified] Ghi chú: catalog liệt `R04 Depends: F04/F05/I01/I03/I07/R02`, tức R02
là tiền điều kiện của R04 dù dòng critical-path tổng hợp ghi `R04/R05/R06` như một
cụm. Tôi trình bày theo đúng thứ tự tiền điều kiện từng issue (R02 trước R04),
không suy diễn thêm ngoài văn bản đã có trong `README.md`.

### R02 — Citation, preview và download authorization

Trích nguyên văn "Remaining for Done" (`README.md` dòng 241-243):
> Remaining for Done: history ACL/IDOR/delete-deny matrices driven only by
> worker-produced artifacts; MinIO cleanup guard soak evidence.

- **Gap 1 — history ACL/IDOR/delete-deny chỉ từ worker-produced artifacts.**
  Test hiện có: `live_citation_authz_expiry_replay_idor_and_immediate_deny`
  trong `crates/server/tests/citation_authz_matrix.rs` (dòng 537), đánh dấu
  `#[ignore = "requires MARKHAND_TEST_DATABASE_URL/APP + MARKHAND_TEST_MINIO_*"]`.
  README nói rõ test này "still SQL-seeds derived artifacts for history ACL
  paths" — tức test đã tồn tại nhưng CHƯA đạt tiêu chí Done (còn seed SQL thay vì
  100% dữ liệu do worker tạo). Đây là việc phải **sửa test/harness**, không phải
  chỉ chạy lại.
  - Lệnh chạy (môi trường live PG/MinIO, theo cấu hình job `rust-integration`
    trong `.github/workflows/ci.yml` dòng 118-124):
    ```bash
    cargo test -p fileconv-server --no-fail-fast -- --include-ignored \
      live_citation_authz_expiry_replay_idor_and_immediate_deny
    ```
    (lệnh suy ra từ mẫu `cargo test -p fileconv-server --no-fail-fast -- --include-ignored --skip e2e_live_vertical_slice`
    ở `.github/workflows/ci.yml` dòng 202-206, thu hẹp bằng tên test cụ thể).
  - Môi trường: live PostgreSQL (admin + `markhand_app` role) + MinIO — không cần
    Qdrant cho test này (không thấy `MARKHAND_TEST_QDRANT_URL` trong ignore
    reason). Có thể dùng `deploy/dev/compose.yml` (`docker compose -f
    deploy/dev/compose.yml up -d postgres qdrant minio` — lệnh nguyên văn từ
    `.github/workflows/ci.yml` dòng 142) dù Qdrant không bắt buộc riêng cho test
    này.
  - Bằng chứng cần commit: test trên chạy xanh SAU KHI đã sửa để không còn SQL
    seed cho history ACL paths — tức output/report của `cargo test` đã sửa, plus
    diff cho thấy path đó nay dùng document/version/artifact do
    `live_upload_convert_index_citation_vertical_slice` (worker-produced) tạo ra
    thay vì fixture SQL.

- **Gap 2 — MinIO cleanup guard soak evidence.** [Unverified] Tôi không tìm thấy
  script hay test nào trong repo có tên khớp "MinIO cleanup guard" (đã tìm trong
  `crates/server/tests/*.rs`, `deploy/scripts/*.sh`, chỉ thấy các khớp không liên
  quan trong `deletion_reconcile.rs` về `orphan_objects`/`orphan_vectors`, đây là
  reconcile-drift test khác, không phải "cleanup guard soak"). **No existing
  command found** cho gap này — đây là bằng chứng chưa có harness, cần viết mới
  (test-to-write), không phải test-to-run. README không trỏ tới file cụ thể nào
  cho mục này.
  - Môi trường dự kiến (suy từ ngữ cảnh O05 soak — không phải trích dẫn trực
    tiếp): Compose POC stack đầy đủ (`deploy/compose.poc.yml`), vì "soak" và
    MinIO cleanup guard nằm cùng phạm vi với O05 (`deploy/scripts/o05-soak.sh`).
    [Speculation] — không có dòng nào trong README/gates.yaml xác nhận môi
    trường chính xác cho gap này, nên tôi không đưa ra lệnh cụ thể.

### R03 — Grounded Q&A, stream và fallback

Trích nguyên văn "Remaining for Done" (`README.md` dòng 262-264):
> Remaining for Done: live router SSE consume + delete-between-batches
> `citation_revoked`; triage-then-current/history matrix on real DB;
> wrong-delta/same-topic contradiction soak through ask path.

- **Gap 1 — live router SSE consume + delete-between-batches `citation_revoked`.**
  Test gần nhất khớp mô tả: `live_ask_is_extractive_and_delete_during_stream_closes`
  trong `crates/server/tests/ask_grounding_matrix.rs` (dòng 230), đánh dấu
  `#[ignore = "requires MARKHAND_TEST_DATABASE_URL/APP + MARKHAND_TEST_MINIO_* + MARKHAND_TEST_QDRANT_URL"]`.
  [Inference] Tên test này khớp một phần mô tả gap ("delete during stream
  closes") nhưng README liệt gap này là CHƯA xong, nên hoặc test chưa bao phủ hết
  "live router SSE consume" (đối lập với đường ask trực tiếp không qua router),
  hoặc còn thiếu case `citation_revoked` cụ thể qua router. Tôi không thể xác
  nhận nội dung chi tiết bên trong hàm test này thêm — cần đọc kỹ để biết còn
  thiếu path nào, tôi chỉ trích được tên test và ignore-reason.
  - Lệnh (môi trường PG+MinIO+Qdrant, khớp job `rust-integration`):
    ```bash
    cargo test -p fileconv-server --no-fail-fast -- --include-ignored \
      live_ask_is_extractive_and_delete_during_stream_closes
    ```
    (cùng mẫu lệnh nguồn như trên).
  - Bằng chứng cần commit: nếu gap là "live router SSE consume" chưa có test
    riêng, đây là **test-to-write** — **no existing command found** cho phần
    "qua router" cụ thể; tôi chỉ xác nhận được test hiện có ở mức ask-path trực
    tiếp.

- **Gap 2 — triage-then-current/history matrix on real DB.** [Unverified] Không
  tìm thấy tên hàm test khớp "triage" trong `crates/server/tests/`. **No
  existing command found.** Đây là test-to-write.

- **Gap 3 — wrong-delta/same-topic contradiction soak through ask path.**
  [Unverified] Không tìm thấy script/test tên khớp "contradiction soak" hay
  "wrong-delta"/"same-topic" trong `bench/markhand_web/soak/` hay
  `crates/server/tests/`. **No existing command found.** Test-to-write, và theo
  từ "soak" có thể cần chạy trong cùng khuôn khổ `deploy/scripts/o05-soak.sh`
  hoặc một soak profile riêng dưới `bench/markhand_web/workloads/` — nhưng tôi
  không thấy file đó tồn tại, nên không suy diễn tên lệnh.

### R04 — Collection/document/job REST API

Trích nguyên văn "Remaining for Done" (`README.md` dòng 285-288):
> Remaining for Done: broader cross-tenant resource IDOR suite beyond
> collections; publish/download/citation HTTP coverage in the same contract
> matrix; full status/schema matrix vs OpenAPI; live Sol R3 barrier evidence on
> CI agent.

- **Gap 1 — broader cross-tenant IDOR beyond collections.** Test hiện có gần
  nhất: `live_http_unauthenticated_and_cross_tenant_are_consistent`
  (`crates/server/tests/api_http_contracts.rs` dòng 1728), `#[ignore = "requires
  MARKHAND_TEST_DATABASE_URL/APP"]`. Đọc nội dung test (dòng 1728-1790+) cho thấy
  nó seed một `foreign_collection/foreign_document/foreign_version/foreign_job/
  foreign_conflict` và test IDOR trên collections/uploads — tức phạm vi hiện tại
  đúng là giới hạn quanh collections/uploads, khớp với "beyond collections" còn
  thiếu (documents/jobs/versions resource riêng lẻ theo route, không chỉ qua
  collection).
  - Lệnh (PG only, không cần MinIO/Qdrant theo ignore-reason):
    ```bash
    cargo test -p fileconv-server --no-fail-fast -- --include-ignored \
      live_http_unauthenticated_and_cross_tenant_are_consistent
    ```
  - Bằng chứng cần commit: mở rộng test này (hoặc thêm test mới cùng file) để
    phủ resource ngoài collection (document/version/job theo ID trực tiếp) —
    **test-to-write** cho phần mở rộng; lệnh trên chỉ chạy được phần đã có.

- **Gap 2 — publish/download/citation HTTP coverage trong cùng contract
  matrix.** Test contract matrix hiện có:
  `live_http_collection_document_job_contract_matrix`
  (`crates/server/tests/api_http_contracts.rs` dòng 460), `#[ignore = "requires
  MARKHAND_TEST_DATABASE_URL/APP + MARKHAND_TEST_MINIO_*"]` — đây là test README
  gọi là "green" hiện tại (đã đóng phần idempotent reindex). Tôi tìm publish/
  download/citation trong cùng file và trong `phase1b_api_contracts.rs`: chỉ
  thấy `citation_pins_are_stable_and_labeled` và
  `download_capability_keys_reject_short_secrets` trong
  `crates/server/tests/phase1b_api_contracts.rs` (dòng 58, 80) — đây là các test
  hermetic/đơn vị riêng lẻ, KHÔNG nằm trong "same contract matrix" như README
  yêu cầu. **No existing command found** cho một contract-matrix hợp nhất
  publish+download+citation — test-to-write, mở rộng
  `live_http_collection_document_job_contract_matrix` hoặc file mới cùng
  pattern.

- **Gap 3 — full status/schema matrix vs OpenAPI.** Có hai test liên quan nhưng
  chỉ là structural/hermetic, không phải "full" matrix:
  - `openapi_lists_business_routes` (`crates/server/tests/phase1b_api_contracts.rs`
    dòng 53)
  - `openapi_route_method_status_parity_is_structural`
    (`crates/server/tests/sse_stream_readiness.rs` dòng 41)
  Lệnh chạy hai test có sẵn (không cần live DB — không có `#[ignore]` gắn ngay
  cạnh theo grep, nên chạy được trong job `rust` thường):
  ```bash
  cargo test -p fileconv-server openapi_lists_business_routes
  cargo test -p fileconv-server openapi_route_method_status_parity_is_structural
  ```
  README gọi gap là "full" matrix — tức hai test cấu trúc này CHƯA đủ; phần còn
  thiếu (đối chiếu từng response status thật với OpenAPI trên live server) là
  **test-to-write**, **no existing command found**.

- **Gap 4 — live Sol R3 barrier evidence on CI agent.** Test khớp nhất về
  "barrier"/advisory lock: `live_write_gate_advisory_lock_concurrency_contract`
  (`crates/server/tests/api_http_contracts.rs` dòng 1539), `#[ignore =
  "requires MARKHAND_TEST_DATABASE_URL/APP + MARKHAND_TEST_MINIO_*"]`, và
  `live_central_write_gate_matrix_refuses_business_side_effects` (dòng 1278,
  cùng ignore-reason). README dùng chữ "Sol R3" (không giải thích thêm trong
  văn bản tôi đọc được) và nói cần "live ... evidence on CI agent" — tức bản
  thân test đã tồn tại, nhưng gap là **chạy trên CI agent thật và giữ bằng
  chứng lại** (log/report), không phải viết test mới.
  ```bash
  cargo test -p fileconv-server --no-fail-fast -- --include-ignored \
    live_write_gate_advisory_lock_concurrency_contract \
    live_central_write_gate_matrix_refuses_business_side_effects
  ```
  Môi trường: job `rust-integration` trong `.github/workflows/ci.yml` (PG admin
  + `markhand_app` role qua `deploy/scripts/bootstrap-server-role.sh`, dòng 194,
  + MinIO qua `deploy/dev/compose.yml`). Bằng chứng cần commit: CI run log/
  artifact cho thấy hai test trên pass trên GitHub Actions agent (không chỉ máy
  local) — [Unverified] tôi không có quyền truy cập lịch sử Actions runs để xác
  nhận đã có run nào pass trên CI agent trước đó; đây là điều cần người vận
  hành xác minh trực tiếp trên GitHub Actions.

### R05 — Search/ask/resumable SSE API

Trích nguyên văn "Gaps remaining for Done" (`README.md` dòng 307-309):
> Gaps remaining for Done: delayed-producer reconnect matrix green on CI agent;
> live purge/load bound evidence; production ask still often extractive when
> entailment fail-closed.

- **Gap 1 — delayed-producer reconnect matrix green on CI agent.** Test khớp
  gần như chính xác: `live_ask_stream_last_event_id_purge_and_delayed_reconnect`
  (`crates/server/tests/sse_stream_readiness.rs` dòng 988), `#[ignore =
  "requires MARKHAND_TEST_DATABASE_URL/APP + MinIO + Qdrant"]`. Tên hàm gộp cả
  "purge" và "delayed_reconnect", nên đây cũng là test khớp Gap 2 bên dưới.
  ```bash
  cargo test -p fileconv-server --no-fail-fast -- --include-ignored \
    live_ask_stream_last_event_id_purge_and_delayed_reconnect
  ```
  Môi trường: PG + MinIO + Qdrant, job `rust-integration`. Bằng chứng cần
  commit: kết quả pass trên CI agent (GitHub Actions runner), không chỉ local —
  cùng giới hạn xác minh như Gap R04.4 ở trên: [Unverified] tôi không xác nhận
  được trạng thái pass/fail lịch sử trên Actions từ trong repo.

- **Gap 2 — live purge/load bound evidence.** Cùng test trên phủ "purge"; cho
  "load bound" có thêm `live_ask_stream_maintenance_converges_under_bounded_load`
  (dòng 1172), `#[ignore = "requires MARKHAND_TEST_DATABASE_URL +
  MARKHAND_TEST_APP_DATABASE_URL"]`.
  ```bash
  cargo test -p fileconv-server --no-fail-fast -- --include-ignored \
    live_ask_stream_maintenance_converges_under_bounded_load
  ```
  Môi trường: chỉ cần PG (admin + app URL), không cần MinIO/Qdrant theo
  ignore-reason.

- **Gap 3 — production ask still often extractive when entailment fail-closed.**
  Đây không phải một test còn thiếu mà là một hành vi sản phẩm được README ghi
  nhận là chưa đạt (GLM grounded entailment thường không khả dụng nên ask rơi
  về extractive fallback). [Unverified] Tôi không tìm thấy trong repo một
  ngưỡng/gate số cụ thể đo "tỷ lệ extractive fallback" — không có trong
  `gates.yaml`. **No existing command found** để đo hay đóng gap này bằng một
  lệnh; đây thuộc phạm vi cấu hình/vận hành nhà cung cấp GLM (ngoài phạm vi
  test), không phải test-to-run hay rõ ràng test-to-write — cần làm rõ với
  retrieval-owner/security-owner (R03 depends on G0-RET/G0-SEC/G1A).

### R06 — OpenAPI, rate limit và readiness

Trích nguyên văn "Gaps remaining for Done" (`README.md` dòng 332-333):
> Gaps remaining for Done: Compose-stack hanging soak on a Docker host.

README mô tả các phần ĐÃ xong ở Sol R2: "hanging probe router matrix
(code+deadline)" — khớp `live_router_trusted_proxy_and_rate_limit_429_metadata`
(`crates/server/tests/sse_stream_readiness.rs` dòng 122, `#[ignore = "requires
MARKHAND_TEST_DATABASE_URL + MARKHAND_TEST_APP_DATABASE_URL"]`) và
`live_health_start_live_ready_contracts` (dòng 198, cùng ignore-reason) — các
test này chạy hermetic/live-DB đơn giản, KHÔNG phải trên toàn bộ Compose stack.

- **Gap — Compose-stack hanging soak on a Docker host.** [Unverified] Tôi không
  tìm thấy trong `deploy/scripts/*.sh` hay `bench/markhand_web/soak/` một script
  nào tên khớp "hanging soak" chạy trên compose stack đầy đủ. Danh sách
  `deploy/scripts/` hiện có (`o02-alert-tabletop.sh`, `o03-bluegreen-restore-drill.sh`,
  `o04-release-suite.sh`, `o05-soak.sh`, `poc-up.sh`, `poc-health.sh`,
  `poc-boot-evidence.sh`, ...) không có mục nào khớp tên. **No existing command
  found.** Đây là test-to-write hoặc một biến thể mới của `o05-soak.sh` nhắm
  riêng vào kịch bản dependency-hanging (timeout của readiness probe khi một
  dependency treo, không phải down hẳn) trên host Docker thật — cùng lớp môi
  trường 24-core host đã dùng cho O-chain
  (`plans/reports/gate-run-260726-2050-markhand-web-phase1b-o-chain-pass.md`
  dòng 6: "24-core Ubuntu 22.04, Docker limited to 10 CPU").
  - Bằng chứng cần commit khi có script: JSON/MD report tương tự các gate O
    khác (`bench/markhand_web/reports/phase-1b-gate/`), ghi rõ compose project,
    image ids, và kết quả readiness dưới điều kiện dependency-hang.

### Tổng hợp môi trường theo issue

| Issue | Môi trường | Nguồn |
|---|---|---|
| R02 (history ACL/IDOR/delete-deny) | Live PG + MinIO (Qdrant không bắt buộc theo ignore-reason) | ignore-attr `citation_authz_matrix.rs:537` |
| R02 (MinIO cleanup guard soak) | [Speculation] Compose POC stack | không có nguồn xác nhận trực tiếp |
| R03 (ask/SSE citation_revoked) | Live PG + MinIO + Qdrant | ignore-attr `ask_grounding_matrix.rs:229` |
| R03 (triage matrix, contradiction soak) | Không xác định — test chưa tồn tại | — |
| R04 (cross-tenant IDOR) | Live PG (app+admin URL) | ignore-attr `api_http_contracts.rs:1727` |
| R04 (contract matrix hiện có) | Live PG + MinIO | ignore-attr `api_http_contracts.rs:459` |
| R04 (write-gate/Sol R3 barrier) | Live PG + MinIO, chạy trên GitHub Actions agent | ignore-attr `api_http_contracts.rs:1538`; job `rust-integration` trong `.github/workflows/ci.yml` |
| R05 (purge/delayed reconnect) | Live PG + MinIO + Qdrant | ignore-attr `sse_stream_readiness.rs:987` |
| R05 (bounded load) | Live PG (app+admin URL) | ignore-attr `sse_stream_readiness.rs:1171` |
| R06 (Compose-stack hanging soak) | 24-core Docker host, Compose POC stack | theo lớp môi trường của O-chain report; không có script riêng xác nhận |

## 2. Kế hoạch O05 với embedding thật

### Vì sao lần pass vừa rồi chưa đo được throughput+quality cùng lúc

`plans/reports/gate-run-260726-2050-markhand-web-phase1b-o-chain-pass.md` dòng
7 ghi: "Embedding: `mock` profile (8 dimensions, deterministic)", và phần
"Scope" (dòng 100-108) nói rõ: "Retrieval quality is not measured here either —
the 8-dimension mock cannot discriminate, so the `G0-RET-*` gates remain the
only evidence for answer quality." Ingest throughput 356 doc/h (dòng 31) vì vậy
chỉ chứng minh capacity của worker pipeline với một embedding rẻ giả lập,
không phải chi phí CPU thật của model.

### Đổi từ mock sang embedding thật

Theo `deploy/.env.example` dòng 8-9, 55-77 và `deploy/compose.poc.yml` dòng
226-260:
- Compose profile: `COMPOSE_PROFILES=mock` (mặc định, `.env.example` dòng 12) →
  đổi sang `COMPOSE_PROFILES=aiteamvn` để bật service `embedding-cpu` (profile
  `["aiteamvn"]`, `deploy/compose.poc.yml` dòng 226-227) thay vì
  `mock-embedding` (profile `["mock"]`, dòng 193-194).
- Biến môi trường đổi theo comment `.env.example` dòng 70-77 (thay khối mock ở
  dòng 60-66):
  ```bash
  MARKHAND_EMBEDDING_BASE_URL=http://embedding-cpu:8080/v1
  MARKHAND_EMBEDDING_API_KEY=poc-embedding-key
  MARKHAND_EMBEDDING_MODEL=AITeamVN/Vietnamese_Embedding
  MARKHAND_EMBEDDING_REVISION=dea33aa1ab339f38d66ae0a40e6c40e0a9249568
  MARKHAND_EMBEDDING_DIMENSIONS=1024
  MARKHAND_EMBEDDING_MAX_SEQ_LENGTH=2048
  MARKHAND_EMBEDDING_BATCH_SIZE=16
  ```
  (`MARKHAND_EMBEDDING_RUNTIME_PATH=local-neural` giữ nguyên — đây chính là ADR
  0005/0006 `runtime_path` cho profile POC/1B, không đổi giữa mock và
  AITeamVN thật vì cả hai đều gắn nhãn `local-neural` trong file .env; do đó tự
  bản thân `runtime_path` không phải là điểm phân biệt mock/thật — điểm phân
  biệt là `MARKHAND_EMBEDDING_MODEL`/`REVISION`/`DIMENSIONS`/`BASE_URL`).
  Có script tải model thật: `deploy/scripts/download-aiteamvn-embedding.sh` (có
  trong `deploy/scripts/` listing) — cần chạy trước khi bật profile `aiteamvn`.
  [Unverified] Tôi chưa đọc nội dung script này để xác nhận chi tiết tham số,
  chỉ xác nhận nó tồn tại theo tên file.

### Hệ quả với index signature / vector rebuild

`docs/adr/0006-index-signature.md` (mục "Decision" điểm 3-4, dòng 24-71): index
signature là SHA-256 length-delimited của `runtime_path`, `embedding_family`
(digest của provider/model/deployment), `embedding_revision`, `dimensions`,
`normalized`, `chunking_version`, `body_text_version`,
`query_normalization_version`. Đổi `MARKHAND_EMBEDDING_MODEL`/`REVISION`/
`DIMENSIONS`/`BASE_URL` từ mock (`markhand-mock`/`poc-local`/8-d) sang AITeamVN
thật (`AITeamVN/Vietnamese_Embedding`/`dea33aa1...`/1024-d) đổi cả
`embedding_family` (digest phụ thuộc base URL) lẫn `embedding_revision` và
`dimensions` → **signature mới, generation mới bắt buộc** theo ADR 0006 điểm 4:
"Changing any signature field creates a new index generation. Mixing vectors
across generations is forbidden; desktop rebuilds on signature mismatch."

Quy trình rebuild tham khảo `docs/runbooks/phase-1b/vector-rebuild.md`:
1. Tính signature mới bằng `deploy/scripts/print-index-signature.py
   --base-url http://embedding-cpu:8080/v1 --model
   AITeamVN/Vietnamese_Embedding --revision
   dea33aa1ab339f38d66ae0a40e6c40e0a9249568 --dimensions 1024` (tham số script
   khớp các flag đọc trực tiếp từ file `deploy/scripts/print-index-signature.py`
   dòng 36-65; giá trị mặc định của script — dòng 46-47, 53-54, 60 — vốn đã là
   AITeamVN/1024-d, tức script này vốn định hướng cho embedding thật, không
   phải mock).
2. Đảm bảo signature đang active được pin (`vector-rebuild.md` bước 1).
3. Rebuild từ PostgreSQL chunks (ADR 0012) nếu Qdrant snapshot không dùng được
   (bước 2).
4. Verify bằng golden retrieval queries only trước khi coi generation mới là
   active (bước 3).
5. Set `MARKHAND_INDEX_SIGNATURE` cho lần chạy O05 mới — biến này đã xuất hiện
   trong `docs/runbooks/phase-1b/soak-o05.md` dòng 226 như một biến môi trường
   bắt buộc của official run ("64 lowercase hex").

### Kỳ vọng và ngưỡng đo

- **Throughput sẽ giảm so với 356 doc/h.** [Inference — không có số cụ thể để
  trích dẫn] `docs/adr/0005-vietnamese-embedding-model-quality.md` mục
  "Consequences" (dòng 64) tự ghi nhận: "CPU throughput ≠ Profile B GPU capacity
  claims; ingest saturation evidence still deferred." Tôi không có bất kỳ số đo
  throughput AITeamVN-CPU-trên-poc-compose nào trong repo để trích dẫn — không
  suy đoán một con số cụ thể theo đúng yêu cầu.
- **Ngưỡng gate áp dụng vẫn là `G0-CAP-INGEST-THROUGHPUT-POC`**
  (`bench/markhand_web/gates.yaml` dòng 270-294): `>= 300` docs/hour,
  `environmentId: poc-compose`, evidence path
  `bench/markhand_web/reports/phase-1b-gate/o05-soak.json`. Notes của gate này
  (dòng 293) nhắc lại: peak tier 1200/hour (`G0-CAP-INGEST-THROUGHPUT`, dòng
  244-267, `environmentId: on-prem-reference`) KHÔNG áp dụng cho poc-compose.
  Ngưỡng latency vẫn `G0-SLO-QUERY-P95 <= 500ms` và `G0-SLO-QUERY-P99 <=
  1000ms` (dòng 296-345) — nhưng cả hai gate SLO này có `environmentId:
  on-prem-reference`, `evidence: null` trong `gates.yaml` hiện tại, nên
  `docs/runbooks/phase-1b/soak-o05.md` bảng "Binding thresholds" (dòng
  129-140) là nơi ràng buộc thực tế các số 500ms/1000ms cho lần chạy O05 —
  không phải qua `on-prem-reference` gate record.
  [Unverified] Tôi không tìm thấy trong repo một gate riêng cho "retrieval
  quality đo cùng lúc với throughput thật" — `G0-RET-*` (recall@5, ndcg-gap,
  temporal/change accuracy, version-citation precision/recall) đều chạy trên
  `environmentId: local-cpu-quality` với corpus/harness riêng
  (`run_retrieval_eval.py`), KHÔNG phải trong O05 soak. Việc "đo throughput và
  retrieval quality cùng lúc" theo yêu cầu #2 của nhiệm vụ này do đó đòi hỏi
  MỘT PHẦN đo bổ sung ngoài các gate hiện có nếu muốn có con số quality trong
  chính lần chạy soak (ví dụ theo dõi citation-hit-rate của các query trong
  `phase1b-mixed.yaml` khi corpus không còn dùng marker/mock 8-d) —
  **no existing command found** cho một gate như vậy; các gate `G0-RET-*` hiện
  hữu là điều kiện tiên quyết riêng biệt, không phải một phần của
  `o05-soak.sh`.
- Lệnh chạy lại O05 (nguyên văn từ `bench/markhand_web/gates.yaml` dòng 283 và
  `docs/runbooks/phase-1b/soak-o05.md` dòng 236):
  ```bash
  bash deploy/scripts/o05-soak.sh --enable-failure-injection --invoke-o03-restore
  ```
  với toàn bộ biến môi trường `MARKHAND_SOAK*`/`MARKHAND_O05_*` như liệt kê ở
  `soak-o05.md` dòng 219-236, cộng thêm `MARKHAND_INDEX_SIGNATURE` mới tính ở
  bước trên, và `COMPOSE_PROFILES=aiteamvn` khi boot stack qua
  `deploy/scripts/poc-up.sh` (dùng trong CI job `phase1b-o04-release-gate`,
  `.github/workflows/ci.yml` dòng 260) thay vì mặc định `mock`.

## 3. Ranh giới Profile B

### Gate chưa có evidence (`bench/markhand_web/gates.yaml`, `evidence: null`)

Tất cả 7 gate sau có `evidence: null` và `environmentId: "on-prem-reference"`:

| Gate id | Metric | Threshold | failureDisposition |
|---|---|---|---|
| `G0-ARCH-DECISIONS` | `approved_architecture_decisions` | `>= 7` | `block-phase-1b` |
| `G0-RET-VLLM-CUTOVER` | `onprem_embedding_cutover_ready` | `>= 1.0` | `block-phase-4` |
| `G0-SLO-QUERY-P95` | `query_latency` p95 | `<= 500ms` | `block-phase-1b` |
| `G0-SLO-QUERY-P99` | `filtered_query_latency` p99 | `<= 1000ms` | `block-phase-1b` |
| `G0-DR-RPO` | `recovery_point` | `<= 15min` | `block-phase-1b` |
| `G0-DR-QUERY-READY-RTO` | `query_ready_recovery_time` | `<= 60min` | `block-phase-1b` |
| `G0-DR-FULL-VECTOR-RTO` | `full_vector_recovery_time` | `<= 240min` | `block-phase-1b` |

Tất cả đều thuộc `environmentId: "on-prem-reference"` — tức đo trên hạ tầng
Profile B thật (không phải `poc-compose` hay `local-cpu-quality`), và hiện
chưa có evidence. `G0-RET-VLLM-CUTOVER` khai `failureDisposition:
block-phase-4` (chặn Phase 4, không phải Phase 1B) — 6 gate còn lại khai
`block-phase-1b`. [Unverified] Bản thân trường `failureDisposition:
block-phase-1b` trong file JSON không tự động nói rõ liệu "Phase 1B đóng" có
nghĩa là các gate này bắt buộc trước khi P1B issues chuyển Done hay chỉ chặn
một mốc release riêng — tôi trích nguyên văn trường này, việc diễn giải chính
sách "block" thuộc thẩm quyền project-owner/architecture-owner.

### Risk register — Critical/High còn mở

`docs/markhand-web-risk-register.md` (toàn bộ 10 dòng của bảng, dòng 8-17),
tất cả đều chưa có dấu hiệu đóng trong file này — dòng 28-29 (mục "Closure
rule") ghi rõ: "This does not close production Phase 0 exit. The
critical/high Profile B blockers above remain active until measured on
`on-prem-reference` with `targetMatch=true`."

| Risk id | Severity | Tóm tắt |
|---|---|---|
| `R-P0-10-DR-01` | Critical | Real PG/MinIO/Qdrant component-loss restore chưa đo trên Profile B |
| `R-P0-10-SCALE-01` | Critical | 20M-vector query P99 + noisy-neighbor chưa đo trên live Qdrant/PostgreSQL |
| `R-P0-10-CAP-01` | High | Peak ingest throughput dựa trên converter smoke, chưa trên target workers/storage |
| `R-P0-10-AUTH-01` | High | Auth/session lifecycle chưa pen-test |
| `R-P0-10-RLS-01` | High | RLS/OrgContext chưa được server migrations enforce |
| `R-P0-10-MIG-01` | High | Cutover AITeamVN → vLLM có thể cần rebuild/rollback storage đầy đủ |
| `R-P0-10-LIC-01` | High | License inventory pass hiện tại, nhưng production packaging có thể thay đổi thành phần |
| `R-P0-10-UPLOAD-01` | High | Upload sandbox/denial mới smoke-tested, chưa chứng minh trong container runtime + malware scanning |
| `R-P0-10-QUEUE-01` | High | Recovery queue age dưới tải 2x mới simulate, chưa đo với OCR/audio/vector workers thật |
| `R-P0-10-MINIO-01` | High | MinIO originals chưa chứng minh reconstructable; object inventory drift risk |

2 Critical + 8 High, tổng 10 dòng — tất cả đứng nguyên "open" theo văn bản file
này tại thời điểm đọc (không có annotation nào trong file cho biết đã đóng
dòng nào sau O-chain pass 2026-07-26).

### Ranh giới rõ ràng

`plans/reports/gate-run-260726-2050-markhand-web-phase1b-o-chain-pass.md` mục
"Scope" (dòng 100-108) tự phát biểu ranh giới, trích nguyên văn: "This
qualifies the single-org POC on `poc-compose` against
`G0-CAP-INGEST-THROUGHPUT-POC` (the SLA normal tier). It makes no Profile B
capacity claim: the peak gate of 1200 documents/hour belongs to
`on-prem-reference`, and this stack caps each worker at 1 CPU and serves
embeddings from a mock. Retrieval quality is not measured here either — the
8-dimension mock cannot discriminate, so the `G0-RET-*` gates remain the only
evidence for answer quality."

Kết hợp với `docs/markhand-web-risk-register.md` dòng 28-29 và
`bench/markhand_web/gates.yaml` (7 gate `on-prem-reference`/`evidence: null`
ở trên), ranh giới Profile B là:

- **Các gate/risk trên chặn PRODUCTION exit** (tức Phase 4 / Profile B rollout),
  không chặn việc đóng 24 issue Phase 1B theo điều kiện đóng ghi ở
  `plans/markhand-web/backlog/phase-1b/issues/README.md` dòng 528-531 ("Phase
  1B chỉ đóng khi 24 issue, mọi external gate, per-format vertical slice, ...
  đều đạt. Release phải được ghi rõ là **trusted single-org POC**") — điều
  kiện đóng Phase 1B không nhắc tới `on-prem-reference` làm bắt buộc, và tự
  văn bản gate-run report đã tuyên bố rõ nó "makes no Profile B claim".
  [Unverified] Tôi không tìm thấy trong `README.md` hay `gates.yaml` một câu
  minh thị nói "on-prem-reference gates không chặn Phase 1B đóng" — kết luận
  này được suy ra từ việc (a) 24-issue/external-gate/vertical-slice list ở
  dòng 528-530 không liệt kê các gate `on-prem-reference` còn thiếu evidence
  làm điều kiện, và (b) risk register + gate-run report tự giới hạn phạm vi
  của mình vào "production"/"Profile B". Đây là suy luận có nguồn nhưng không
  phải trích dẫn trực tiếp một câu duy nhất — gắn nhãn [Inference].
- **O-chain pass không đưa ra bất kỳ tuyên bố Profile B nào** — xác nhận trực
  tiếp từ chính report, không cần suy diễn thêm.
