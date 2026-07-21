# Review chất lượng Phase 1B đợt 2 — Markhand Web

Ngày review: 2026-07-20
Phạm vi: code merge vào `master` từ đợt review trước, dải commit
`b7147ae..0ca74bb` (~40 commit) — các issue Phase 1B **F04, F05, F06,
I01, I02, I03, I04, I05, I06**, cùng việc đối chiếu lại 8 khuyến nghị của
báo cáo [`code-review-260719-1327`](code-review-260719-1327-markhand-web-done-tasks-report.md).

Phương pháp: 5 luồng review độc lập đọc code tại HEAD, đối chiếu acceptance
criteria trong issue catalog + docs (upload-policy, threat-model, ADR),
chạy test khi khả thi (`cargo test -p fileconv-server --lib` → 94 pass;
`build-roadmap.py --check` → pass, 43 done). Mọi finding đều kèm file:line
đã xác minh trực tiếp trong code.

## Kết luận tổng quát

**Chất lượng code Phase 1B rất cao** — cao hơn mặt bằng POC thông thường.
Các cơ chế khó (RLS transaction-local, saga idempotent, fencing token,
sandbox namespace/Landlock, chống zip-bomb, quota atomic) được làm bài bản
và có test adversarial thật (barrier race, fault-injection theo từng điểm
crash, allocator tracking bounded-memory). Backlog trung thực: F02 vẫn để
`blocked`, không có "done ảo".

Nhưng có **hai vấn đề xuyên suốt** làm giảm độ tin cậy:

1. **Toàn bộ test integration không chạy trong CI** — lỗ hổng critical của
   đợt trước (Finding 1) *chưa* được sửa, chỉ đổi từ "skip êm giả pass"
   sang `#[ignore]`. Mọi bằng chứng acceptance của F03–I06 là chạy tay
   cục bộ, không được verify liên tục.
2. **Chưa xử lý gần hết khuyến nghị đợt trước** — trong 8 khuyến nghị:
   0 xử lý trọn vẹn, 3 một phần, 5 chưa động đến. 40 commit dồn vào feature
   mới (I01–I06) chứ không phải nợ kỹ thuật.

| Issue | Verdict |
|---|---|
| F04 — OrgContext / repositories / state machine | **Pass** — tenancy fail-closed, RLS không leak qua pool, CAS transition atomic |
| F05 — Password auth / rotating sessions / bearer | **Pass** nhưng có **1 security finding major** + ADR thiếu |
| F06 — Fail-closed PG/Qdrant/MinIO adapters | **Pass** — filter org/collection bắt buộc, key opaque, ID xác định |
| I01 — Streaming quarantine upload | **Pass** — bounded-memory thật, chống zip-bomb; lệch policy vài chỗ |
| I02 — Atomic quota admission | **Pass** — không double-spend, clock DB sau lock |
| I03 — Durable jobs / outbox / event log | **Pass** — fencing + sequencing per-org đúng |
| I04 — Isolated converter worker | **Pass** — sandbox thật, fail-closed preflight |
| I05 — Idempotent promotion saga | **Pass** — crash-matrix gần đủ, TOCTOU đã đóng |
| I06 — Chunk/embedding/index worker | **Pass** — visibility gate on durable embeddings đúng |

## Phát hiện theo mức độ

### Critical

1. **CI vẫn không chạy test integration** (lặp lại Finding 1 đợt trước, *chưa
   sửa*). `.github/workflows/ci.yml` không có service Postgres/MinIO/Qdrant,
   không set `MARKHAND_TEST_DATABASE_URL`, không truyền `--ignored`. Tất cả
   test DB trong `schema_migrations.rs`, `auth.rs`, `repositories.rs`,
   `quota.rs`, `uploads.rs`, `jobs.rs`, `worker.rs`, `index_worker.rs`,
   `storage.rs` đều `#[ignore]`. Một regression trong RLS/fencing/saga vẫn
   merge được với CI xanh. Đây là điều kiện tiên quyết trước khi xây R01–R06.

### Major (security)

2. **Forge refresh token → inject audit log + oracle org tồn tại**
   (`crates/server/src/auth/session.rs:503`, `:886`). Token có dạng
   `mh1.<org_uuid>.<secret>`; `org_id` lấy từ token của kẻ tấn công, dùng
   set RLS GUC, và khi token lạ thì ghi một dòng deny vào `audit_log` của
   *chính org đó*. Kẻ chưa xác thực biết org UUID (trả về trong mọi
   login/`/me`, không coi là bí mật) có thể — trước cả rate-limit (R06 chưa
   làm) — bơm không giới hạn dòng deny vào audit log append-only của tenant;
   và org UUID không tồn tại làm insert vi phạm FK → trả 500 thay vì 401,
   qua đó phân biệt org thật/giả. Cần verify token *trước* khi set GUC/ghi
   audit theo org do client cung cấp.

### Major (khác)

3. **Server vẫn không có logging/tracing** (Finding 5 đợt trước, chưa sửa).
   `crates/server/Cargo.toml` không có `tracing`/`log`/otel; readiness path
   (`http.rs`) vẫn tính chuỗi lỗi rồi vứt (`Result<(), ()>`). Readiness flap
   production vẫn vô hình với operator.
4. **P1B-F02 vẫn chưa làm** — không có `deploy/Dockerfile.server`,
   `Dockerfile.worker`, `compose.poc.yml`, không có hardening container.
   Isolation được làm ở tầng in-process (sandbox.rs) nhưng cgroup memory
   limit chỉ best-effort, phụ thuộc F02 để có cap thật. Status catalog
   trung thực (blocked), nhưng I04/I05/I06 done dù phụ thuộc F02.

### Medium

5. **Reserve quota *sau* khi nhận toàn bộ body** (`routes/uploads.rs:68`,
   `services/upload/mod.rs`) — ngược thứ tự policy §3 (reserve trước
   stream). Tenant hết quota vẫn tiêu 200 MiB disk/băng thông mỗi request.
6. **Đếm trang PDF bypass được** (`services/upload/mod.rs:387`) — scan byte
   thô tìm `/Type /Page`, không thấy page trong `/ObjStm` nén → page bomb
   lọt qua giới hạn 500 trang ở tầng intake.
7. **Idempotency-Key thành 409 vĩnh viễn sau settlement** (`routes/uploads.rs:93`,
   `services/quota.rs:798`) — retry upload đã fail/expire/finalize với cùng
   key không bao giờ thành công lại; không phải lỗi bảo mật, là lỗi
   availability/contract.
8. **Preflight CSV/HTML chỉ đọc 1 MiB đầu** + CSV bỏ dòng đầu
   (`mod.rs:675`, `:700`) — script/formula sau 1 MiB hoặc ở dòng 1 lọt qua.
9. **Qdrant payload flags (`is_current`/`is_effective`) stale sau promotion**
   (`workers/embedding.rs:251`) — không có đường cập nhật/xóa point cũ
   (`delete_by_scope` không có caller ngoài test). R01 *bắt buộc* không được
   dùng flag payload làm bộ lọc current-version duy nhất; phải hydrate lại
   từ PG. Chờ I07 reconcile.
10. **Outbox relay nghẽn head-of-line bởi poison event** (`jobs/mod.rs:746`)
    — không có attempt counter/dead-letter cho outbox event; một event bị
    sink từ chối vĩnh viễn chặn mọi event sau đó của org.
11. **Shutdown không graceful cho conversion đang chạy** (`bin/worker.rs:137`)
    — `tokio::select!` drop future đang chạy; sandbox trong `spawn_blocking`
    không bị dừng, process exit bỏ qua `Drop` → có thể orphan sandbox child
    trên host (giảm nhẹ khi chạy trong container — mà F02 chưa có).

### Minor / observation (chọn lọc)

12. Manifest checksum migration: script Python có verify SHA-256 và có trong
    CI (đã có từ trước review), nhưng **phía Rust vẫn chỉ so tên file**
    (`database.rs:220`); không có guard thứ tự append-only (Finding 2, gần
    như chưa đổi).
13. Seed POC 0011 **vẫn chạy vô điều kiện** mọi lần start, chưa gate profile
    (Finding 3 nửa vời) — nhưng seed tooling đã hợp nhất về
    `deploy/scripts/seed-dev-all.sh` và tài liệu hoá hai account là chủ đích.
14. **Web chưa đụng tới** (Finding 7, 0 commit vào `web/`): chưa bật
    StrictMode, mock `Response` vẫn dùng chung instance, vẫn 2 test.
15. `fileconv-core` chưa tách feature — server/knowledge vẫn build whisper.cpp
    (Recommendation 6). Giảm nhẹ vì I04 worker giờ có dùng converter của core.
16. **`CORPUS.md` vẫn ghi sai** 29 docs/260 queries (thực tế 31/268) — nửa
    còn lại của Finding 13; `gates.yaml` đã sửa, P0-05 đã đóng với evidence
    chain nhất quán (catalog ↔ gates ↔ summary.json ↔ ADR 0005 ↔ runbook).
17. F05 minor: keyspace advisory lock 32-bit `hashtext` (collision); lộ
    credential-validity cho user thiếu membership (403 vs 401 sau khi verify
    đúng mật khẩu); logout không vô hiệu access token đang sống (≤15 phút —
    ADR chấp nhận).
18. F05 thiếu ADR ghi quyết định transport bearer (phase-2 plan vẫn mô tả
    đường cookie+CSRF) → Phase 2 có thể mở lại câu hỏi transport.

## Điểm đã xác minh là tốt

- **Tenancy (F04)**: mọi method nghiệp vụ public đều nhận `&OrgContext` +
  predicate `org_id`; RLS set qua `set_config(is_local=true)`, test chứng
  minh cùng `pg_backend_pid` trên pool size-1 và `app.org_id` rỗng sau cả
  commit lẫn rollback; cross-org deny chứng minh bằng raw SQL.
- **Auth (F05)**: Argon2id tham số OWASP, rehash-on-login, dummy verify
  chống timing; JWT HS256 chặn `alg:none`/sai alg/iss/aud/kid/exp cả trước
  và sau decode; refresh token opaque 256-bit lưu SHA-256, reuse revoke cả
  family; secret không lọt Debug/log/audit.
- **Saga (I05)**: promote là một transaction fenced bằng lease; crash-matrix
  chứng minh idempotent ở gần như mọi điểm (fault-injection assert 4 chiều
  leak: versions/artifacts/outbox/quota); TOCTOU staging-key đã đóng bằng
  key theo từng claim (hash lease token).
- **Index (I06)**: visibility chỉ bật khi embedding đã bền trong Qdrant
  (`wait=true`) rồi transition trong một txn PG fenced; signature xác định,
  dùng chung `EmbeddingPlan::index_signature` với desktop (chống drift).
- **Upload (I01)**: streaming 64 KiB thật (test allocator peak < 32 MiB cho
  body 64 MiB), chống zip-bomb đếm entry từ EOCD trước khi giải nén, ratio
  100:1 theo từng entry, cancellation-safe qua owned task; 9/10 adversarial
  fixture được test.
- **Roadmap/backlog**: `build-roadmap.py --check` pass, 43 done; các done
  mark đều có implementation thật + commit trail hardening; runbook mới
  chính xác 100% khi spot-check lệnh make/compose/curl.

## Đề xuất ưu tiên

1. **Thêm CI job có service Postgres/MinIO/Qdrant chạy `cargo test -p
   fileconv-server -- --include-ignored`** với `MARKHAND_TEST_DATABASE_URL`
   trỏ role non-superuser. Đây là việc quan trọng nhất — mở khoá độ tin cậy
   cho toàn bộ acceptance evidence Phase 1B (finding 1).
2. **Vá lỗ audit-log injection**: verify refresh token trước khi set GUC /
   ghi audit theo org do client cung cấp; trả 401 đồng nhất cho org
   không tồn tại (finding 2).
3. Thêm `tracing` + log lý do readiness fail (finding 3).
4. Dead-letter/attempt counter cho outbox relay (finding 10) trước khi sink
   làm việc thật.
5. Reserve quota trước khi nhận body, hoặc cập nhật policy cho khớp thực tế;
   quarantine PDF không đếm được trang qua `/ObjStm`; bỏ `.skip(1)` khi quét
   formula CSV (finding 5, 6, 8).
6. Ghi ADR quyết định transport bearer; sửa `CORPUS.md`; gate seed 0011 theo
   profile (finding 18, 16, 13).
7. Làm P1B-F02 (Dockerfile + compose.poc + cgroup cap) — cần cho cả memory
   limit thật lẫn graceful shutdown an toàn (finding 4, 11).

## Nguồn

Năm báo cáo review chi tiết (F04/F05, F06/I06, I01/I02, I03/I04/I05,
audit follow-up) tổng hợp trong session ngày 2026-07-20; mọi finding kèm
file:line đối chiếu trực tiếp tại commit `0ca74bb`.
