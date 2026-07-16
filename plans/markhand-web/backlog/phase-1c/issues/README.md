# Phase 1C issues — Multi-org security

Parent plan: [`../../../phase-1c-multi-org-security.md`](../../../phase-1c-multi-org-security.md)

Mọi issue ở trạng thái **Backlog**, activate sau Phase 1B gate.

## Dependency

```text
1C-01 → 1C-02 → 1C-05 → 1C-06 → 1C-07 ─┐
   └──→ 1C-03 → 1C-04 ──────────────────┤
ADR RLS ───────→ 1C-08 ─────────────────┤
1C-09 → 1C-10 → 1C-11 ─────────────────┴→ 1C-12 → 1C-13
```

## 1C-01 — Organization lifecycle và validated context

- **Plan/files:** Org create/list/detail/switch, service/repo/middleware; issue new
  context/session after verified membership.
- **Depends:** Phase 1B auth/schema. **Acceptance/tests:** Chỉ thấy org của mình;
  forged/stale header deny; two-org resolver/integration tests.
- **Security/migration:** Không global org state; audit switch. **Out:** billing/OIDC.

## 1C-02 — Membership, invites và last-owner invariant

- **Plan/files:** Hashed single-use invite; membership state; transactional last-owner;
  membership version; session revoke. MVP chưa có mail dùng invite URL/token hiển thị
  đúng một lần cho admin copy qua kênh được tổ chức phê duyệt; expiry/revoke/audit
  bắt buộc.
- **Depends:** 1C-01. **Acceptance/tests:** Không remove/downgrade last owner; admin
  không quản owner; concurrent owner removal, invite replay/expiry, escalation tests.
- **Security/migration:** Row lock, expand/backfill version; plaintext invite không
  lưu DB/log. **Out:** automated email delivery/SCIM/MFA.

## 1C-03 — Canonical RBAC seed

- **Plan/files:** Permission constants + DB seed owner/admin/editor/viewer; immutable
  system roles; OpenAPI fixture.
- **Depends:** Phase 1B role schema. **Acceptance/tests:** Matrix đúng/idempotent,
  duplicate/missing/immutable mutation tests; UI không hard-code matrix.
- **Security/migration:** Stable keys, expand/backfill. **Out:** custom role builder.

## 1C-04 — Route/service guards và service identities

- **Plan/files:** Deny-by-default `authorize`; apply route+service+worker/reconcile;
  least-privilege identities.
- **Depends:** 1C-01/03. **Acceptance/tests:** Allow/deny mỗi permission cả hai layer;
  missing-guard inventory, direct-service và worker misuse tests.
- **Security/migration:** Không `internal=true` bypass. **Out:** generic ABAC.

## 1C-05 — Collection ACL resolver/cache

- **Plan/files:** Private/org/groups grants; ACL/version snapshot; cache key org/user/
  membership/ACL version; invalidation APIs.
- **Depends:** 1C-02/03. **Acceptance/tests:** Semantics đúng, empty/error fail closed;
  grants/status/cache/revoke tests.
- **Security/migration:** Backfill ACL version. **Out:** nested/time-based groups.

## 1C-06 — PostgreSQL ACL enforcement

- **Plan/files:** Tenant+ACL predicates cho list/count/autocomplete/FTS/hydration.
- **Depends:** 1C-05. **Acceptance/tests:** Không path thiếu context; no existence/count
  leak; SQL join/subquery/missing-predicate tests.
- **Security/migration:** PG authority, prepared queries. **Out:** vector/object path.

## 1C-07 — Qdrant/storage/jobs fail-closed enforcement

- **Plan/files:** Mandatory org+non-empty collection filter; PG payload validation;
  authorize preview/download/export/job/SSE; abort in-flight on ACL change.
- **Depends:** 1C-05/06. **Acceptance/tests:** Missing/malformed/timeout/mismatch deny;
  Qdrant failure, forged payload, job ID, stream revoke, signed URL replay tests.
- **Security/migration:** No signed URL logs. **Out:** public sharing/CDN.

## 1C-08 — RLS và pool defense

- **Plan/files:** Nếu ADR chọn: FORCE RLS, non-owner app role, transaction-local context,
  worker role, pool reset/verification.
- **Depends:** ADR + 1C-01/06. **Acceptance/tests:** No owner/BYPASSRLS; wrong/missing/
  pooled-context/worker misuse/migration tests.
- **Security/migration:** Expand policy trước force; nếu không chọn, close bằng ADR
  + repository evidence. **Out:** thay app guards bằng RLS.

## 1C-09 — Atomic quota lifecycle

- **Plan/files:** Reserve/finalize/refund, idempotency/expiry/sweeper/reconcile cho
  storage/token/jobs.
- **Depends:** Phase 1B jobs + 1C-01. **Acceptance/tests:** 100 concurrent reservations
  không over-limit; crash/retry/cancel/timeout/actual-usage tests.
- **Security/migration:** Checked arithmetic, org/resource unique key. **Out:** billing.

## 1C-10 — Rate limit và per-org fairness

- **Plan/files:** User/IP/auth limits, per-org worker/GPU scheduler/semaphore, headers,
  privacy-safe metrics.
- **Depends:** 1C-09 + Phase 0 SLO/capacity. **Acceptance/tests:** Noisy org không phá
  SLO org khác; burst/window/fair-load/crash-release/proxy tests.
- **Security/migration:** Chỉ trusted proxy IP, bounded state. **Out:** multi-region.

## 1C-11 — Audit/admin APIs

- **Plan/files:** Member/role/ACL/config/quota/data/cloud events; read-only pagination/
  filter/retention; owner-only controls.
- **Depends:** 1C-02…10. **Acceptance/tests:** Mọi mutation có actor/org/action/target/
  result/request ID; coverage/access/pagination/redaction/retention tests.
- **Security/migration:** No document/prompt/token/PII/URL. **Out:** SIEM archive.

## 1C-12 — Multi-org denial suite

- **Plan/files:** Fixture 2 org, ≥3 users, duplicate names, private/org/groups, stale
  token; phủ list/count/FTS/vector/Q&A/citation/preview/download/export/jobs/SSE/
  cache/audit/worker/reconcile/in-flight revoke.
- **Depends:** 1C-01…11. **Acceptance/tests:** Zero content/metadata/existence leak,
  route + direct service, CI + deployed environment.
- **Security/migration:** Deployment-like roles, exploit-first regression.
  **Out:** external pentest.

## 1C-13 — Security/revoke/load gate

- **Plan/files:** Token/revoke/cache/Qdrant partial/reconcile/quota/noisy-neighbor/
  supply-chain suite + gate report.
- **Depends:** 1C-10/11/12. **Acceptance/tests:** Leakage 0; revoke bound; quota recovery;
  fairness SLO; audit complete; no undispositioned high/critical.
- **Security/migration:** Record environment/threshold/approver. **Out:** SPA/OIDC.

## Exit gate

Chỉ đóng Phase 1C khi 1C-12 và 1C-13 xanh cả CI lẫn deployed environment. Đây là
gate trước khi cho nhiều org khác trust boundary và trước khi Phase 2 hoàn tất.
