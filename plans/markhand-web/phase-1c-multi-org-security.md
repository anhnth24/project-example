# Phase 1C — Multi-org, RBAC/ACL và quota

## Outcome

Mở kiến trúc single-org POC thành multi-org an toàn. Phase này hoàn thiện policy,
fairness và denial suite; không retrofit `org_id` vì tenancy primitives đã có từ 1B.

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

Permission constants:

- `doc.upload`, `doc.delete`;
- `qa.query`;
- `member.manage`, `settings.manage`;
- `intel.use`, `pii.manage`, `export.run`;
- `audit.view`.

Role→permission lưu DB, system role immutable; schema cho custom role tương lai nhưng
chưa cần UI editor phức tạp. Mỗi route và service operation có explicit guard.
Worker/admin/reconcile cũng dùng service identity với permission tối thiểu.

Canonical built-in matrix:

| Permission | Owner | Admin | Editor | Viewer |
|---|---:|---:|---:|---:|
| `doc.upload` | ✓ | ✓ | ✓ | |
| `doc.delete` | ✓ | ✓ | own/explicit policy | |
| `qa.query` | ✓ | ✓ | ✓ | ✓ |
| `member.manage` | ✓ | ✓, không quản owner | | |
| `settings.manage` | ✓ | ✓, trừ owner/security | | |
| `intel.use` | ✓ | ✓ | ✓ | theo org policy |
| `pii.manage` | ✓ | ✓ | | |
| `export.run` | ✓ | ✓ | ✓ | theo org policy |
| `audit.view` | ✓ | ✓ | | |

Chỉ owner được assign/remove owner, đổi security/SSO policy, xóa org và thay quota
hard limit. Admin không được nâng chính mình hoặc người khác lên owner. Test allow/
deny phải phủ mỗi permission ở route lẫn service layer; matrix có migration seed và
fixture duy nhất, không hard-code bản thứ hai trong UI.

## P1C.3 — Collection ACL

Chốt semantics từ ADR:

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

Flow transaction:

```text
reserve → finalize(actual) | refund
```

Resource:

- upload/storage bytes;
- LLM/embedding tokens;
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

- Member/role/ACL/config/quota changes.
- Upload/delete/export/PII/cloud-LLM use.
- Authorization deny và quota exceed.
- Read-only audit endpoint có pagination/filter và `audit.view`.
- Retention được config; log không chứa document text, prompt, token hoặc PII.

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
