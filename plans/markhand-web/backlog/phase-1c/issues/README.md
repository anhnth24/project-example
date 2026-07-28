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

- **Status:** In progress — nửa *validated-context* đã có + test DB-gated: resolver `auth/permissions.rs:39`, membership re-verify (JWT chỉ là hint) `auth/middleware.rs:144`, fail-closed `auth/context.rs:30`, RLS `migrations/0002`. **Nửa lifecycle đã landed phần lớn**: `GET /orgs` (chỉ org của mình), `GET /orgs/{id}` (404 đồng nhất cho "không tồn tại"/"không phải member" — không oracle), `POST /orgs/switch` (re-verify membership từ PG, mint session mới độc lập scoped target org, audit `org.switch` cả success/deny; deny với org không tồn tại thì không ghi audit để tránh FK-oracle) — routes bearer-identity-only theo tiền lệ `accept_invite`, kèm two-org resolver test DB-gated (`tests/orgs.rs`, 8 test: forged/stale/suspended deny + audit). **Còn thiếu**: `POST /orgs` (create) — CHẶN bởi câu hỏi thiết kế 1C-03 (role seed cho org mới: template canonical copy per-org, hay roles catalog toàn cục + `role_permissions` per-org?); org mới tạo hôm nay sẽ không có role nào.

- **Plan/files:** Org create/list/detail/switch, service/repo/middleware; issue new
  context/session after verified membership.
- **Depends:** Phase 1B auth/schema. **Acceptance/tests:** Chỉ thấy org của mình;
  forged/stale header deny; two-org resolver/integration tests.
- **Security/migration:** Không global org state; audit switch. **Out:** billing/OIDC.

## 1C-02 — Membership, invites và last-owner invariant

- **Status:** Backlog — chỉ có **schema** (`org_invites` hashed single-use `migrations/0003:93`, `org_memberships` `0001:26`). KHÔNG có service/route invite (create/accept/revoke/expiry), KHÔNG có last-owner invariant (grep rỗng), không có membership state/version, `member.manage` seed nhưng chưa dùng. Session-family revoke có (`auth/session.rs:862`) nhưng thuộc auth 1B.

- **Plan/files:** Hashed single-use invite; membership state; transactional last-owner;
  membership version; session revoke. MVP chưa có mail dùng invite URL/token hiển thị
  đúng một lần cho admin copy qua kênh được tổ chức phê duyệt; expiry/revoke/audit
  bắt buộc.
- **Depends:** 1C-01. **Acceptance/tests:** Không remove/downgrade last owner; admin
  không quản owner; concurrent owner removal, invite replay/expiry, escalation tests.
- **Security/migration:** Row lock, expand/backfill version; plaintext invite không
  lưu DB/log. **Out:** automated email delivery/SCIM/MFA.

## 1C-03 — Canonical RBAC seed

- **Status:** In progress — bảng `permissions/roles/role_permissions` + seed 4 role owner/admin/editor/viewer + matrix (`migrations/0011:38`, `is_system=true`), idempotent (`tests/schema_migrations.rs` DB-gated). **Thiếu**: seed POC-only gắn 1 org hardcode (không seed cho org mới), không trigger immutable system-role, không OpenAPI fixture, không test kiểm giá trị matrix.

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

- **Status:** In progress — resolver cơ bản (org/private/owner/`collection_user_access`) + fail-closed `services/retrieval/mod.rs:181` + test. **Thiếu đúng phần 1C**: grant theo `groups`/`role` chưa resolve (bảng có, không ai đọc), KHÔNG có ACL-version/snapshot, KHÔNG có cache, KHÔNG có invalidation API. `context.rs:11` tự ghi full ACL thuộc 1C.

- **Plan/files:** Private/org/groups grants; ACL/version snapshot; cache key org/user/
  membership/ACL version; invalidation APIs.
- **Depends:** 1C-02/03. **Acceptance/tests:** Semantics đúng, empty/error fail closed;
  grants/status/cache/revoke tests.
- **Security/migration:** Backfill ACL version. **Out:** nested/time-based groups.

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

- **Status:** In progress — `audit_log` append-only (trigger immutability `migrations/0026`) + writer co-commit trong txn mutation (`services/audit.rs:447`) + redaction forbidden-keys + test tốt (DB-gated + unit). **Nhưng nửa admin = 0**: KHÔNG có endpoint audit/member/role/usage nào (openapi grep rỗng), hàm đọc `db/audit.rs:11 list_recent` **chưa được gọi** và không phân trang/filter, không có code quản membership, không có audit action cho member/role/config, không có retention.

- **Plan/files:** Member/role/ACL/config/quota/data/cloud events; read-only pagination/
  filter/retention; owner-only controls.
- **Depends:** 1C-02…10. **Acceptance/tests:** Mọi mutation có actor/org/action/target/
  result/request ID; coverage/access/pagination/redaction/retention tests.
- **Security/migration:** No document/prompt/token/PII/URL. **Out:** SIEM archive.

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
