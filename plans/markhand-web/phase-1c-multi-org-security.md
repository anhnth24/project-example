# Phase 1C — Multi-org, RBAC/ACL và quota

## Outcome

Mở kiến trúc single-org POC thành multi-org an toàn. Phase này hoàn thiện policy,
fairness và denial suite; không retrofit `org_id` vì tenancy primitives đã có từ 1B.

**PR 1 progress (2026-07-31):** 1C-01 / 1C-02 / 1C-03 Done with exact-SHA CI evidence
on `a62850422dd070e7e1195bfe1d4f1dee0d73566d` (run
[30629207747](https://github.com/anhnth24/project-example/actions/runs/30629207747);
jobs `rust` / `web` / `rust-integration`).

**PR 2 progress (2026-07-31):** 1C-05 / 1C-06 Done with exact-SHA CI evidence on
`90742281e51d3c8ca8a32a78077a07fe3449bc68` (run
[30649044974](https://github.com/anhnth24/project-example/actions/runs/30649044974);
jobs changes/static / `rust` / `rust-integration`). Task 4 review Approved; Task 5 review
Approved; Task 6 rereview Approved. Audit retention stays deferred under
`AR-1C-AUDIT-RETENTION`
(POC/non-production only; expires before production multi-org or Phase 4 gate).
Qualifying embedding remains local/mock until embedding-token metering exists.

## P1C.1 — Organization và membership

- Tạo/join/switch org.
- Invite, activate, suspend và remove member.
- Role assignment owner/admin/editor/viewer.
- Quy tắc owner cuối cùng không thể tự xóa/hạ role.
- Membership version tăng khi role/ACL đổi để invalidate cache/session.
- Org context lấy từ route/header đã validate với membership, không tin claim do
  client tự chọn.

API gồm org list/detail, members CRUD và org switch/session refresh.

## P1C.2 — RBAC level 2

**Disposition (PR 1):** P1C.2: active/reserved matrix follows
`crates/server/openapi/builtin-role-catalog.json`. That fixture is the sole normative
built-in contract; the table below is a human summary and must not diverge from it.
Role→permission runtime authority remains PostgreSQL; OpenAPI/web consumers reference
the same fixture (no second hard-coded grant matrix).

Active permission keys (seeded when operations exist):

- `doc.upload`, `doc.delete`, `doc.publish`, `doc.quarantine.review`;
- `qa.query`, `qa.history`;
- `member.manage`, `audit.view`, `jobs.system`.

Reserved permission keys (ungranted until a real operation activates them):

- `settings.manage`, `intel.use`, `pii.manage`, `export.run`.

System roles immutable; schema may allow custom roles later without a complex UI
editor. Each route and service operation has an explicit guard. Worker/admin/reconcile
use least-privilege service identities.

Canonical built-in matrix (active grants only; matches `builtin-role-catalog.json`):

| Permission | Owner | Admin | Editor | Viewer | Notes |
|---|---:|---:|---:|---:|---|
| `doc.upload` | ✓ | ✓ | ✓ | | |
| `doc.delete` | ✓ | ✓ | | | Editor “own/explicit” deferred; not a built-in grant |
| `doc.publish` | ✓ | ✓ | ✓ | | Active; was missing from older plan prose |
| `doc.quarantine.review` | | | | | Active with zero default grants (intentional) |
| `qa.query` | ✓ | ✓ | ✓ | ✓ | |
| `qa.history` | ✓ | ✓ | | | |
| `member.manage` | ✓ | ✓ | | | Admin cannot manage owner (`admin_cannot_manage_owner`) |
| `audit.view` | ✓ | ✓ | | | |
| `jobs.system` | ✓ | ✓ | | | Active; was missing from older plan prose |
| `settings.manage` | | | | | Reserved / ungranted |
| `intel.use` | | | | | Reserved / ungranted |
| `pii.manage` | | | | | Reserved / ungranted |
| `export.run` | | | | | Reserved / ungranted |

Chỉ owner được assign/remove owner, đổi security/SSO policy, xóa org và thay quota
hard limit. Admin không được nâng chính mình hoặc người khác lên owner. Test allow/
deny phải phủ mỗi *active* permission ở route lẫn service layer; matrix có migration
seed và fixture duy nhất, không hard-code bản thứ hai trong UI. Guard inventory for
active operations belongs to later PRs (1C-04), not the catalog seed itself.

## P1C.3 — Collection ACL

**Disposition (PR 2):** P1C.3 PostgreSQL collection ACL semantics are **resolved** on
`90742281e51d3c8ca8a32a78077a07fe3449bc68` (run
[30649044974](https://github.com/anhnth24/project-example/actions/runs/30649044974)):
canonical `(qa.query, read)` resolver projection; `private`/`org`/`groups` visibility with
group/role grant branches and `read`/`write`/`admin` access-level rank; migration `0036`
dormant-grant rejection; containment revoke preserving other user grants; resolver↔SQL
equivalence matrix with explicit oracle assertions; shared `db/acl_sql` predicates on
FTS/hydration/conflict paths with dual `qa.query` + `qa.history` historical recheck;
upload/quarantine operation-scoped write guards (no read-projection inference).

Remaining P1C.3 surfaces stay with later issues — not reopened here:

- Qdrant filter enforcement (1C-07);
- document list/count/autocomplete when built (must use shared predicates from day one);
- preview/download/export/job/SSE beyond current PostgreSQL paths;
- broader route write inventory (PR 3).

Machine-checked semantics delivered in PR 2:

- `private`: owner + principal được grant;
- `org`: thành viên org có permission tương ứng;
- `groups`: principal group/user được grant.

Enforce tại:

- document list/count/autocomplete;
- PG FTS;
- Qdrant filter;
- citation hydration;
- preview/download/export;
- job status/SSE/reindex/delete;
- cache key.

Qdrant adapter fail closed khi:

- thiếu org;
- thiếu/empty allowed collections;
- filter malformed;
- ACL resolution timeout;
- payload tenant không khớp PG.

## P1C.4 — RLS và repository defense

Nếu ADR Phase 0 chọn RLS:

- bật và `FORCE ROW LEVEL SECURITY` cho bảng tenant;
- application role không own/bypass policy;
- set org context theo transaction, reset khi trả pool;
- worker role tách riêng, audit mọi cross-scope operation.

Dù có RLS, repository vẫn bắt buộc `OrgContext`; RLS là lớp thứ hai, không thay
application authorization.

## P1C.5 — Atomic quota và fairness

**Disposition (PR 1):** P1C.5: embedding-token metering is N/A only for local/mock
qualifying runtime. Phase 1C qualification must use local/mock embedding; cloud/shared
embedding profiles are out of scope until embedding-token metering exists. LLM/chat
token reserve/finalize remains in scope where a chat provider is configured.

Flow transaction:

```text
reserve → finalize(actual) | refund
```

Resource:

- upload/storage bytes;
- LLM tokens (when a chat provider is configured);
- embedding tokens — N/A for local/mock qualifying runtime only (see disposition);
- concurrent convert/embed/intelligence jobs;
- request rate.

Yêu cầu:

- reservation có expiry/sweeper;
- crash/retry/cancel không leak quota;
- LLM finalize bằng usage thật;
- 429 trả quota headers và retry hint;
- semaphore/scheduler per-org để noisy neighbor không chiếm toàn worker/GPU;
- rate limit per-user, per-IP cho unauthenticated và chặt hơn ở auth endpoints.

## P1C.6 — Audit/admin APIs

**Disposition (PR 1):** P1C.6: audit retention is deferred to Phase 4 under
`AR-1C-AUDIT-RETENTION`. Phase 1C keeps append-only audit without configurable
purge/TTL; Phase 4 owns retention, tamper evidence, and export. Accepted risk scope
is POC/non-production only. AR expiry: before production multi-org or Phase 4 gate,
whichever comes first.

- Member/role/ACL/config/quota changes.
- Upload/delete/export/PII/cloud-LLM use.
- Authorization deny và quota exceed.
- Read-only audit endpoint có pagination/filter và `audit.view`.
- Retention/TTL deferred (see disposition / `AR-1C-AUDIT-RETENTION`); log không chứa
  document text, prompt, token hoặc PII.

## P1C.7 — Denial suite

Fixture tối thiểu:

- 2 org;
- ít nhất 3 user/org;
- private/org/groups collections;
- document/collection trùng tên giữa org;
- token cũ trước khi revoke.

Chứng minh không rò qua:

- list, count, search, FTS, Qdrant;
- Q&A và citation;
- preview, download, export;
- reindex/delete/job/SSE;
- autocomplete, error và existence side-channel;
- cache sau org switch;
- signed URL;
- audit;
- worker/reconcile/admin code path;
- in-flight Q&A sau ACL revoke.

Database test:

- thiếu/sai org;
- join/subquery thiếu tenant predicate;
- pool connection context leakage;
- RLS bypass;
- privileged worker role misuse.

Quota race test chạy ít nhất 100 concurrent reservations ở sát limit và chứng minh
finalized usage không vượt policy.

## P1C.8 — Security/load validation

- Noisy-neighbor: một org ingest nặng, org khác vẫn đạt latency/fairness budget.
- Token rotation/reuse/revoke.
- ACL cache invalidation.
- Qdrant timeout/partial failure phải fail closed.
- Reconciliation không vượt scope.
- Dependency/container vulnerability scan.

## Gate

- Denial suite pass trong CI và environment deploy thật.
- Cross-tenant leakage bằng 0.
- Quota reconcile đúng sau crash, timeout, retry và cancellation.
- Membership/ACL revoke có hiệu lực trong bound đã chốt.
- Noisy-neighbor vẫn trong SLO.
- Mọi administrative action có audit.
- Chỉ sau gate này mới cho nhiều org/người dùng không cùng trust boundary.

## Không thuộc phase

- Web SPA hoàn chỉnh.
- Custom-role builder nâng cao.
- OIDC/group sync.
- Billing thương mại.
