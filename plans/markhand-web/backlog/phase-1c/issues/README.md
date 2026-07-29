# Phase 1C issues — Multi-org security

Parent plan: [`../../../phase-1c-multi-org-security.md`](../../../phase-1c-multi-org-security.md)

<!-- roadmap-default-status: backlog -->

**Audit 2026-07-27 (đọc code, không đoán).** Con số cũ "0/13" gây hiểu nhầm: phần lớn
**nền tảng thực thi 1C đã được xây sẵn trong Phase 1B** (RLS FORCE, ACL predicate ở
retrieval, Qdrant mandatory filter, quota atomic, rate-limit, audit append-only) — nhưng
**chưa issue nào đạt full acceptance**, nên không tick Done cái nào. Kết quả audit:

- **11 In progress** — có building block thật + có test, nhưng thiếu phần định nghĩa 1C.
- **2 Backlog** — chỉ có schema/prose, logic chính chưa viết: **1C-02** (invite/last-owner)
  và **1C-13** (security/load gate).
- **0 Done.**

> **Hai ranh giới quan trọng, áp cho MỌI issue dưới đây:**
> 1. **Test enforcement gần như toàn bộ là `#[ignore]` DB-gated** — chỉ chạy trong job CI
>    `rust-integration` khi có `MARKHAND_TEST_DATABASE_URL`/MinIO/Qdrant, KHÔNG chạy trong
>    `cargo test` mặc định. Fast-unit chỉ phủ logic thuần (scope resolve, filter, HMAC).
> 2. **`context.rs:11-12` tự ghi "full ACL resolution belongs to Phase 1C"** — substrate
>    hiện tại xây cho single-org POC (1B), chưa phải multi-org đầy đủ.
>
> **Exit gate Phase 1C (1C-12 + 1C-13) CHƯA đạt** — cả hai deliverable "suite gắn kết" và
> "gate có report" đều chưa tồn tại như deliverable. Và Phase 1C chỉ activate sau khi
> **Phase 1B gate đóng** (R04/R06 còn pending), nên toàn phase vẫn là công việc phía trước.

## Dependency

```text
1C-01 → 1C-02 → 1C-05 → 1C-06 → 1C-07 ─┐
   └──→ 1C-03 → 1C-04 ──────────────────┤
ADR RLS ───────→ 1C-08 ─────────────────┤
1C-09 → 1C-10 → 1C-11 ─────────────────┴→ 1C-12 → 1C-13
```

## 1C-01 — Organization lifecycle và validated context

- **Status:** In progress — nửa *validated-context* đã có + test DB-gated: resolver `auth/permissions.rs:39`, membership re-verify (JWT chỉ là hint) `auth/middleware.rs:144`, fail-closed `auth/context.rs:30`, RLS `migrations/0002`. **Nửa lifecycle đã landed phần lớn**: `GET /orgs` (chỉ org của mình), `GET /orgs/{id}` (404 đồng nhất cho "không tồn tại"/"không phải member" — không oracle), `POST /orgs/switch` (re-verify membership từ PG, mint session mới độc lập scoped target org, audit `org.switch` cả success/deny; deny với org không tồn tại thì không ghi audit để tránh FK-oracle) — routes bearer-identity-only theo tiền lệ `accept_invite`, kèm two-org resolver test DB-gated (`tests/orgs.rs`, 8 test: forged/stale/suspended deny + audit). **`POST /orgs` (create) đã landed** sau khi owner chốt thiết kế catalog toàn cục (xem 1C-03/migration `0030`): tạo org + owner membership + `provision_org_role_catalog(org_id)` trong MỘT transaction, audit `org.create`, validate slug khớp CHECK constraint, rate-limit auth-IP, 409 slug taken; test DB-gated create (happy/duplicate/unauth/audit + owner resolve đủ permission không cần seed riêng) trong `tests/orgs.rs` (12 test).

- **Plan/files:** Org create/list/detail/switch, service/repo/middleware; issue new
  context/session after verified membership.
- **Depends:** Phase 1B auth/schema. **Acceptance/tests:** Chỉ thấy org của mình;
  forged/stale header deny; two-org resolver/integration tests.
- **Security/migration:** Không global org state; audit switch. **Out:** billing/OIDC.

## 1C-02 — Membership, invites và last-owner invariant

- **Status:** In progress — gần Done; **đã landed đầy đủ trong #317** (đọc code xác
  minh 2026-07-28, không phải backlog cũ ghi "Backlog"): invite create/accept/revoke/list
  hashed single-use `routes/members.rs` E1-E5, **`PATCH /api/v1/members/{userId}`** (đổi
  role/state, `routes/members.rs:450 patch_member`) và **`DELETE /api/v1/members/{userId}`**
  (`routes/members.rs:551 delete_member`) đều đã có — không chỉ GET như một đợt audit
  trước từng nêu. Last-owner invariant transactional + row-lock: `check_last_owner_invariant`
  (pure, `services/members.rs:143`) + `guard_last_owner` (`FOR UPDATE` qua
  `db::members::lock_owner_rows`, `services/members.rs:159`) chạy cùng transaction với
  `change_role`/`remove_member`/`suspend_member` (`services/members.rs:476/540`); riêng
  cả tự-hạ-role/tự-xóa của owner cuối cũng 409 (không có ngoại lệ cho "chính mình").
  `guard_owner_tier` (`services/members.rs:176`) chặn thêm escalation: chỉ active owner
  mới được cấp/thao tác owner khác — admin có `member.manage` không tự thăng owner hay
  quản owner khác (403). Membership **state** (`active|suspended`, KHÔNG có "removed" —
  remove là hard-delete có chủ đích vì FK `refresh_tokens` không có `ON DELETE`, xem
  migration `0029_expand_org_membership_state.sql` + doc `services/members.rs:25-35`)
  đã dùng cho suspend/reactivate; **version** cho ACL cache (1C-05) cố ý CHƯA thêm (chưa
  có cache để invalidate). Downgrade/suspend/remove đều gọi
  `auth::session::revoke_all_user_families` (route layer, `routes/members.rs:532/587`)
  để refresh-token family cũ không sống quá TTL. Audit `member.role_change` (old→new
  role/state trong metadata) và `member.remove` (old_role) ghi cùng transaction
  (`services/audit.rs` allowlist `member.role_change`/`member.remove`). OpenAPI
  (`openapi/openapi.yaml:1180`, `ROUTE_INVENTORY`/`BODY_TAKING_OPERATIONS` trong
  `api/openapi.rs`) + web codegen + admin UI (`web/src/components/admin/membersApi.ts`,
  `memberPresentation.ts`) đã có, pin-count schema vẫn 38 (không schema mới). Migration
  MỚI không cần — `0029` đã đủ.
  **Bổ sung trong phiên này** (worktree, chưa chạy DB-gated CI): thêm vào
  `tests/members.rs` 4 test còn thiếu so với danh sách acceptance — last-owner 409
  *deterministic* (không chỉ qua race) cho tự-downgrade/tự-remove/tự-suspend của owner
  cuối cùng, 403 khi thiếu `member.manage` (kèm audit-deny), 404 cho user id chưa từng
  tồn tại trong org (khác case cross-org RLS-hidden), và audit before/after cho
  role-change + remove trên happy path.
  **Còn thiếu thật** (không phải của 1C-02, thuộc issue khác): 1C-05 chưa có ACL
  version/cache nên chưa cần cột `version`; 1C-11 vẫn chưa có endpoint đọc audit log
  (chỉ ghi, `db::audit::list_recent` chưa có route). MVP token invite vẫn chỉ hiển thị
  1 lần qua response body — automated email delivery vẫn out of scope như thiết kế.

- **Plan/files:** Hashed single-use invite; membership state; transactional last-owner;
  membership version (deferred to 1C-05); session revoke. MVP chưa có mail dùng invite
  URL/token hiển thị đúng một lần cho admin copy qua kênh được tổ chức phê duyệt;
  expiry/revoke/audit bắt buộc — **tất cả đã landed**.
- **Depends:** 1C-01. **Acceptance/tests:** Không remove/downgrade last owner (kể cả tự
  thao tác chính mình); admin không quản owner; concurrent owner removal, invite
  replay/expiry, escalation tests — **đã có trong `tests/members.rs`, DB-gated
  `#[ignore]`, cần chạy trong CI `rust-integration` (`MARKHAND_TEST_DATABASE_URL`) để
  đóng gate, chưa tự chạy trong `cargo test` mặc định**.
- **Security/migration:** Row lock (`FOR UPDATE` trên owner rows), expand/backfill
  version (deferred, xem trên); plaintext invite không lưu DB/log (chỉ trả 1 lần trong
  response). **Out:** automated email delivery/SCIM/MFA.

## 1C-03 — Canonical RBAC seed

- **Status:** In progress — bảng `permissions/roles/role_permissions` + seed 4 role owner/admin/editor/viewer + matrix (`migrations/0011:38`, `is_system=true`), idempotent (`tests/schema_migrations.rs` DB-gated). **Đã thêm (migration `0030`)**: catalog toàn cục `role_catalog`/`role_catalog_permissions` làm template bất biến duy nhất (trigger chặn UPDATE/DELETE/TRUNCATE — immutable system roles), seed idempotent đúng matrix hiệu lực POC, hàm `provision_org_role_catalog(org_id)` copy per-org khi tạo org (cố ý KHÔNG trigger tự động trên `orgs` INSERT và KHÔNG đổi resolver join — per-org rows vẫn là nguồn resolve runtime để `acl_mutate` revoke containment hoạt động); test matrix + immutability + no-seed-resolve trong `tests/role_catalog.rs`. **Còn thiếu**: OpenAPI fixture cho matrix, đối chiếu UI không hard-code matrix.

- **Plan/files:** Permission constants + DB seed owner/admin/editor/viewer; immutable
  system roles; OpenAPI fixture.
- **Depends:** Phase 1B role schema. **Acceptance/tests:** Matrix đúng/idempotent,
  duplicate/missing/immutable mutation tests; UI không hard-code matrix.
- **Security/migration:** Stable keys, expand/backfill. **Out:** custom role builder.

## 1C-04 — Route/service guards và service identities

- **Status:** In progress — deny-by-default `require_permission` (`auth/permissions.rs:118`) áp ở **cả route lẫn service** (nhiều endpoint), DB role least-priv migrator/app (`migrations/0027/0028`), test unit + DB-gated (`citation_authz_matrix.rs:537`). **Thiếu**: guard ở tầng worker/reconcile (worker chạy với **permission rỗng** `bin/worker.rs:153`, chỉ có tenant scope), không có missing-guard inventory (ROUTE_INVENTORY chỉ là parity method/path), chưa phủ allow/deny cho doc.delete/publish/member.manage/audit.view/jobs.system.

- **Plan/files:** Deny-by-default `authorize`; apply route+service+worker/reconcile;
  least-privilege identities.
- **Depends:** 1C-01/03. **Acceptance/tests:** Allow/deny mỗi permission cả hai layer;
  missing-guard inventory, direct-service và worker misuse tests.
- **Security/migration:** Không `internal=true` bypass. **Out:** generic ABAC.

## 1C-05 — Collection ACL resolver/cache

- **Status:** In progress — resolver cơ bản (org/private/owner/`collection_user_access`) +
  fail-closed `services/retrieval/mod.rs:181` + test, **và giờ có version + cache** (phiên
  này, worktree, chưa chạy DB-gated CI): `orgs.acl_version` (migration
  `0031_expand_org_acl_version.sql`, cột `bigint NOT NULL DEFAULT 1`, expand-only) là version
  **org-wide** (cố ý không per-membership/per-role — `revoke_role_permission_for_principal`
  xoá một hàng `role_permissions` dùng chung bởi mọi member cùng role, nên version hẹp hơn
  sẽ under-invalidate; over-invalidate cả org là hướng an toàn, chấp nhận được ở quy mô
  POC). `db::orgs::bump_acl_version` được gọi trong CÙNG transaction với
  `services::members::{change_role, suspend_member, reactivate_member (qua set_member_state),
  remove_member}` và `services::acl_mutate::{revoke_role_permission_for_principal,
  revoke_collection_access_for_principal}` — đúng tập mutation duy nhất ảnh hưởng
  `auth::permissions::resolve_org_context*` hiện có (đã audit toàn bộ call site, xem báo
  cáo phiên). Cache in-process `auth::context_cache::OrgContextCache` (bounded, mặc định
  4096 principal / TTL 3s, field mới trên `auth::provider::PasswordAuthProvider`, không đổi
  signature `PasswordAuthProvider::new`) bọc **đúng một** điểm vào: `AuthenticatedOrg`
  extractor (`auth/middleware.rs`) — KHÔNG bọc `resolve_org_context_on_txn` (ask-stream
  append/pull, cần snapshot không bị xé trong transaction đã khoá) và KHÔNG bọc
  `services::upload::saga::reload_principal_locked` (saga tự re-check trước commit) — cả
  hai đường này vẫn luôn-tươi như trước, không có rủi ro hồi quy từ cache. Mỗi cache hit
  (trong TTL) BẮT BUỘC một freshness-check rẻ (`users.disabled_at` + `orgs.acl_version`,
  2 bảng không RLS) trước khi tin dữ liệu cache — mismatch/disabled/lỗi truy vấn đều rơi về
  resolve đầy đủ (fail-closed, không tự chế deny). Vì mọi hit đều hỏi lại PostgreSQL (không
  bao giờ tin cache "mù" theo TTL), một version bump ở tiến trình khác lập tức thấy được ở
  request tiếp theo của tiến trình này — nên **không cần cơ chế invalidation cross-instance
  riêng** (câu hỏi mở nêu trong nhiệm vụ đã tự đóng nhờ thiết kế). TTL chỉ còn là lưới an
  toàn cho (a) call site tương lai quên bump và (b) KHOẢNG TRỐNG ĐÃ BIẾT: tạo/xoá-mềm
  collection qua `db::collections` (ngoài `services::acl_mutate`) **chưa** bump version —
  cố ý để ngoài phạm vi phiên này (xem "Out" bên dưới).
  Test DB-gated mới `tests/acl_cache.rs` (5 test): role-downgrade/suspend/remove qua route
  HTTP thật (cùng bearer token, không re-login) đều 403 ngay ở request kế tiếp; ACL
  collection revoke (gọi thẳng `OrgContextCache::resolve` như extractor thật) drop quyền
  ngay; và một test tách riêng cơ chế version-check (bump `acl_version` thủ công, không qua
  helper) để chứng minh chính cơ chế mismatch-detection hoạt động độc lập với nơi gọi nó.
  Unit test thuần (không DB) trong `auth::context_cache` phủ `trusts_cache`/
  `should_check_freshness`/bounded-eviction/`clear`. `tests/members.rs`,
  `tests/uploads.rs`, `tests/sse_stream_readiness.rs`, `tests/orgs.rs`,
  `tests/schema_migrations.rs` chạy lại xanh không đổi hành vi (xem kết quả verify trong
  báo cáo phiên).
  **Cố tình KHÔNG làm trong phiên này** (xem báo cáo phiên để biết lý do đầy đủ): (1)
  grant theo `groups`/`role` vẫn CHƯA resolve (bảng có, không ai đọc) — đây là một tính
  năng authz mới (semantics resolve qua `groups`/`group_memberships`), không phải cache
  key, cần thiết kế/review riêng, không phải phạm vi "version + cache" của vòng này; (2)
  version bump cho `db::collections::{insert,soft_delete,update_metadata}` — cần audit
  call-site + test riêng, ghi nhận là khoảng trống đã biết bên trên, giới hạn bởi TTL cho
  đến khi đóng; (3) cache/version không (chưa) operator-configurable qua env — hằng số
  module `DEFAULT_CAPACITY`/`DEFAULT_TTL`, có thể nâng cấp thành config sau nếu cần.

- **Plan/files:** Private/org/groups grants (groups **vẫn thiếu**, xem trên); ACL/version
  snapshot (**done**: `orgs.acl_version`, migration `0031`); cache key org/user/membership/
  ACL version (**done**: `auth::context_cache::OrgContextCache`, key `(org_id, user_id)` +
  version check); invalidation APIs (**không làm riêng** — invalidation là version-bump
  trong transaction mutation, không phải API endpoint mới; không có yêu cầu nào đòi một API
  invalidation tách biệt).
- **Depends:** 1C-02/03. **Acceptance/tests:** Semantics đúng, empty/error fail closed
  (pre-existing, không đổi); grants/status/cache/revoke tests (**done**: `tests/acl_cache.rs`
  + unit tests `auth::context_cache`).
- **Security/migration:** Backfill ACL version (**done**: `DEFAULT 1`, expand-only,
  migration `0031`). **Gap version-bump đã ĐÓNG (migration `0033`, 2026-07-29)**: trigger
  DB `bump_org_acl_version()` trên `collections`/`collection_user_access`/
  `org_memberships`/`roles`/`role_permissions` — bump cùng transaction cho MỌI writer,
  kể cả SQL trực tiếp (fixtures/vận hành; CI `rust-integration` bắt được đúng lỗ này
  ở `api_http_contracts`/`citation_authz_matrix` trước khi vá). **Out:**
  nested/time-based groups; groups/role-based grant resolution (tính năng mới, không
  phải phần "cache" — xem trên); operator-configurable cache capacity/TTL qua env.

## 1C-06 — PostgreSQL ACL enforcement

- **Status:** In progress — tenant+ACL predicate trên FTS/hydration/conflict-evidence (`db/search.rs:312/486`), list/count tenant-scoped, test DB-gated + fast. **Thiếu**: autocomplete (không tồn tại), nhánh FTS candidate chưa mang ACL subquery (dựa re-check ở hydration), count chưa có ACL predicate, không unit test missing-predicate riêng.

- **Plan/files:** Tenant+ACL predicates cho list/count/autocomplete/FTS/hydration.
- **Depends:** 1C-05. **Acceptance/tests:** Không path thiếu context; no existence/count
  leak; SQL join/subquery/missing-predicate tests.
- **Security/migration:** PG authority, prepared queries. **Out:** vector/object path.

## 1C-07 — Qdrant/storage/jobs fail-closed enforcement

- **Status:** In progress — Qdrant mandatory org+collection filter (`storage/qdrant.rs:897`), PG payload validation, download capability authorize+replay (`services/download.rs`), authorize preview/download/export/job/SSE, abort in-flight → `citation_revoked` (`services/qa/ask_stream.rs`); test tốt (fast + DB/MinIO/Qdrant-gated). **Thiếu**: vài fast-unit deny (forged/timeout Qdrant client); signed-URL không áp dụng — thay bằng capability token (theo thiết kế).

- **Plan/files:** Mandatory org+non-empty collection filter; PG payload validation;
  authorize preview/download/export/job/SSE; abort in-flight on ACL change.
- **Depends:** 1C-05/06. **Acceptance/tests:** Missing/malformed/timeout/mismatch deny;
  Qdrant failure, forged payload, job ID, stream revoke, signed URL replay tests.
- **Security/migration:** No signed URL logs. **Out:** public sharing/CDN.

## 1C-08 — RLS và pool defense

- **Status:** In progress — FORCE RLS ~26 bảng (`migrations/0010`), app role không owner/không BYPASSRLS (`0027/0028`), GUC transaction-local `set_config('app.org_id',…,true)` (`db/pool.rs:94`), assert FORCE-RLS ở startup (`database.rs:309`), test pool-leak/cross-org/force-rls/deploy-role (DB-gated). **Thiếu**: pool reset/DISCARD/verify runtime lúc checkout, DB role riêng cho worker (worker dùng chung `markhand_app`).

- **Plan/files:** Nếu ADR chọn: FORCE RLS, non-owner app role, transaction-local context,
  worker role, pool reset/verification.
- **Depends:** ADR + 1C-01/06. **Acceptance/tests:** No owner/BYPASSRLS; wrong/missing/
  pooled-context/worker misuse/migration tests.
- **Security/migration:** Expand policy trước force; nếu không chọn, close bằng ADR
  + repository evidence. **Out:** thay app guards bằng RLS.

## 1C-09 — Atomic quota lifecycle

- **Status:** In progress — reserve/finalize/refund + reserve_upload hai-tài-nguyên atomic (`services/quota.rs`), idempotency theo key, expiry, sweeper **đã wire** vào background (`http.rs:367`), checked arithmetic, advisory lock; 11 test DB-gated + unit. **Thiếu**: token-quota lifecycle (`Tokens` định nghĩa nhưng `ask` không reserve), concurrent-jobs enforce ở prod (test-only), quota reconcile, test mới đạt 16 concurrent (acceptance đòi ≥100).

- **Plan/files:** Reserve/finalize/refund, idempotency/expiry/sweeper/reconcile cho
  storage/token/jobs.
- **Depends:** Phase 1B jobs + 1C-01. **Acceptance/tests:** 100 concurrent reservations
  không over-limit; crash/retry/cancel/timeout/actual-usage tests.
- **Security/migration:** Checked arithmetic, org/resource unique key. **Out:** billing.

## 1C-10 — Rate limit và per-org fairness

- **Status:** In progress — limiter per-IP/user/route (`middleware/rate_limit.rs`) + Retry-After header + metrics privacy-safe + worker type-fairness (`workers/index.rs:137`). **Thiếu đúng phần định nghĩa 1C-10**: **per-ORG fairness** (không có bucket per-org — key org chỉ là prefix per-user; không có scheduler/semaphore GPU per-org; worker chạy **1 org/tiến trình** `bin/worker.rs:151`), không có test noisy-neighbor/SLO, không có GPU scheduler.

- **Plan/files:** User/IP/auth limits, per-org worker/GPU scheduler/semaphore, headers,
  privacy-safe metrics.
- **Depends:** 1C-09 + Phase 0 SLO/capacity. **Acceptance/tests:** Noisy org không phá
  SLO org khác; burst/window/fair-load/crash-release/proxy tests.
- **Security/migration:** Chỉ trusted proxy IP, bounded state. **Out:** multi-region.

## 1C-11 — Audit/admin APIs

- **Status:** In progress — audit **READ** endpoint landed (phiên này, worktree,
  chưa chạy DB-gated CI): `GET /api/v1/audit` (`routes/audit.rs`), org-scoped
  (`org_id = $1` defense-in-depth + RLS `audit_log_org_isolation` migration `0010`
  là gate thật), guard `audit.view` (đã seed sẵn `migrations/0011`/`0030`, owner/admin
  có) — 403 khi thiếu, và **tự ghi audit `audit.read`/deny cho chính lần đọc bị từ
  chối** (đọc audit log đủ nhạy cảm để audit cả deny lẫn success, giống pattern
  `search.query`/`ask.query`, không giống `list_members`/`list_invites`/`usage`
  không audit gì). Cursor pagination `(created_at, id)` DESC ổn định — cùng convention
  `db::documents::list_in_collection` (tái dùng `api::pagination::{encode_cursor,
  decode_cursor}` sẵn có, không thêm cơ chế mới). Filter: `action` (exact, validate
  qua `AuditAction::parse` — 400 nếu không thuộc enum đóng), `actor` (uuid),
  `from`/`to` (RFC3339, validate `from <= to`); limit mặc định 50 / max 100 (route
  clamp qua `Pagination::from_query`, `db::audit::list_page` clamp 101 để chừa chỗ
  cho hàng dò-thêm phát hiện `hasMore` đúng tại limit=100). Hàm mới `db::audit::list_page`
  (giữ nguyên `list_recent` cũ — vẫn được gọi trực tiếp trong `tests/members.rs`).
  OpenAPI: path `/audit` + schema `AuditEntry`/`AuditPage` mới trong `openapi.yaml`,
  `ROUTE_INVENTORY`/`BODY_TAKING_OPERATIONS` cập nhật trong `api/openapi.rs`; web
  codegen (`pnpm --dir web api:generate`) + pin-count schema 38→40 (2 test web) +
  `pnpm --dir web test` xanh (446/446, xem ghi chú dưới về 1 fail không liên quan).
  Test DB-gated mới `tests/audit_read.rs` (8 test, chạy xanh cục bộ với Postgres 16
  local): happy list + cursor pagination không gap/không trùng qua nhiều trang,
  filter theo action/actor/time-range, 403 thiếu `audit.view` **và** audit deny row
  đó phải đọc lại được bởi người có quyền, org isolation 2-org (org B không thấy
  actor/email của org A), và metadata trả về không vượt allowlist hiện có của từng
  action (vd `member.invite` chỉ `{reason, invite_id, role}`, không bao giờ email).
  **Sửa 1 chỗ status cũ đã stale**: dòng trên từng ghi "KHÔNG có endpoint audit/
  member/role/usage nào (openapi grep rỗng)" — sai, `GET /members`, `GET /members/
  invites`, `GET /usage` (và CRUD member) đã landed từ 1C-02 (`routes/members.rs`,
  xem mục 1C-02 ở trên); chỉ riêng **đọc audit log** là thật sự thiếu trước phiên này.
  **Ngoài phạm vi issue-con "đọc audit log" này** (xem `Out` bên dưới): action set
  KHÔNG đổi ngoài `audit.read` mới — không thêm audit action nào cho ACL/config/
  quota/cloud event (đó là ghi audit ở nơi khác, không phải endpoint đọc); retention/
  archival không làm; owner-only admin *mutation* controls (member/role/ACL/config/
  quota khác) không thuộc "đọc log" nên không đụng.

- **Plan/files:** Member/role/ACL/config/quota/data/cloud events (**out of scope của
  đợt này — chỉ audit READ**); read-only pagination/filter (**done**: `routes/audit.rs`,
  `db/audit.rs::list_page`)/retention (**out**); owner-only controls (**out — không
  phải đọc log**).
- **Depends:** 1C-02…10. **Acceptance/tests:** Mọi mutation có actor/org/action/target/
  result/request ID (pre-existing, không đổi ở đây); coverage (pre-existing, không
  đổi)/access (**done**: 403 + deny-audit test)/pagination (**done**: cursor stable
  test)/redaction (**done**: no-leak-beyond-allowlist test)/retention (**out of
  scope**, chưa làm).
- **Security/migration:** No document/prompt/token/PII/URL (allowlist per-action giữ
  nguyên, `audit.read` chỉ thêm `result_count`). Không migration mới — RLS/seed
  permission đã có sẵn. **Out:** SIEM archive, retention/TTL, audit coverage mở
  rộng sang action mới cho ACL/config/quota/cloud mutation, owner-only admin
  controls khác ngoài đọc log.

## 1C-12 — Multi-org denial suite

- **Status:** In progress — có ~10 cross-org check **rải rác** (list/count, vector, jobs, SSE, worker, RLS/pool-leak, in-flight ACL revoke) đều `#[ignore]` DB-gated. **Deliverable "suite gắn kết" CHƯA có**: không có fixture 2-org/≥3-user/duplicate-name/groups/stale-token; các mặt **FTS, Q&A, preview/download, export, cache** CHƯA có test cross-org; chưa chạy trên deployed environment (acceptance đòi CI **và** deployed).

- **Plan/files:** Fixture 2 org, ≥3 users, duplicate names, private/org/groups, stale
  token; phủ list/count/FTS/vector/Q&A/citation/preview/download/export/jobs/SSE/
  cache/audit/worker/reconcile/in-flight revoke.
- **Depends:** 1C-01…11. **Acceptance/tests:** Zero content/metadata/existence leak,
  route + direct service, CI + deployed environment.
- **Security/migration:** Deployment-like roles, exploit-first regression.
  **Out:** external pentest.

## 1C-13 — Security/revoke/load gate

- **Status:** Backlog — gate có tên **KHÔNG tồn tại**: không có 1C gate trong `bench/markhand_web/gates.yaml` (toàn `G0-*`, disposition `block-phase-1b`), không có 1C gate report trong `plans/reports/` (chỉ có report 1B), không có 1C CI job (chỉ `phase1b-o04-release-gate`). **Noisy-neighbor** và **supply-chain/container scan** chưa có gì (chỉ prose + 1 rủi ro mở `R-P0-10-SCALE-01` chưa đo).

- **Plan/files:** Token/revoke/cache/Qdrant partial/reconcile/quota/noisy-neighbor/
  supply-chain suite + gate report.
- **Depends:** 1C-10/11/12. **Acceptance/tests:** Leakage 0; revoke bound; quota recovery;
  fairness SLO; audit complete; no undispositioned high/critical.
- **Security/migration:** Record environment/threshold/approver. **Out:** SPA/OIDC.

## Exit gate

Chỉ đóng Phase 1C khi 1C-12 và 1C-13 xanh cả CI lẫn deployed environment. Đây là
gate trước khi cho nhiều org khác trust boundary và trước khi Phase 2 hoàn tất.
