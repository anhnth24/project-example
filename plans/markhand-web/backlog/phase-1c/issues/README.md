# Phase 1C issues — Multi-org security

Parent plan: [`../../../phase-1c-multi-org-security.md`](../../../phase-1c-multi-org-security.md)

<!-- roadmap-default-status: backlog -->

**Audit 2026-08-01 (PR 3 guard/identities).** Substrate 1B vẫn là nền; PR 1–2 đóng
org/RBAC/ACL; PR 3 đóng CI-verifiable route/service guards, Qdrant/storage fail-closed,
quota/fairness, và audit read. `AR-1C-AUDIT-RETENTION` (owner-approved PR 1) giữ
POC/non-production only — retention/TTL **chưa** implement. Deployed POC worker
qualification vẫn thuộc PR 5.

- **10 Done** — **1C-01**…**1C-03**, **1C-05**, **1C-06**, **1C-04**, **1C-07**,
  **1C-09**, **1C-10**, **1C-11** (enforcement closures on `6833f57d94949c75ea36609e1055a1139e097c8a`, run
  [30678318560](https://github.com/anhnth24/project-example/actions/runs/30678318560): `rust`
  [91310110938](https://github.com/anhnth24/project-example/actions/runs/30678318560/job/91310110938), `rust-integration`
  [91310110925](https://github.com/anhnth24/project-example/actions/runs/30678318560/job/91310110925)).
- **2 In progress** — **1C-08** (CI half complete; deployed half → PR 5), **1C-12**
  (connected multi-org denial suite).
- **1 Backlog** — **1C-13** (security/load gate).

> **Hai ranh giới quan trọng, áp cho MỌI issue dưới đây:**
> 1. **Test enforcement gần như toàn bộ là `#[ignore]` DB-gated** — chỉ chạy trong job CI
>    `rust-integration` khi có `MARKHAND_TEST_DATABASE_URL`/MinIO/Qdrant, KHÔNG chạy trong
>    `cargo test` mặc định. Fast-unit chỉ phủ logic thuần (scope resolve, filter, HMAC).
> 2. **`context.rs:11-12` tự ghi "full ACL resolution belongs to Phase 1C"** — substrate
>    hiện tại xây cho single-org POC (1B); full multi-org ACL semantics vẫn thuộc PR 2+.
>
> **Exit gate Phase 1C (1C-12 + 1C-13) CHƯA đạt** — denial suite gắn kết và security/load
> gate chưa đóng. Phase 1B gate đã đóng (2026-07-31 — R06 hanging soak pass).

## Dependency

```text
1C-01 → 1C-02 → 1C-05 → 1C-06 → 1C-07 ─┐
   └──→ 1C-03 → 1C-04 ──────────────────┤
ADR RLS ───────→ 1C-08 ─────────────────┤
1C-09 → 1C-10 → 1C-11 ─────────────────┴→ 1C-12 → 1C-13
```

## 1C-01 — Organization lifecycle và validated context

- **Status:** Done — CI exact-SHA evidence on `a62850422dd070e7e1195bfe1d4f1dee0d73566d`
  (run [30629207747](https://github.com/anhnth24/project-example/actions/runs/30629207747)):
  `rust` [91151657403](https://github.com/anhnth24/project-example/actions/runs/30629207747/job/91151657403),
  `web` [91151657388](https://github.com/anhnth24/project-example/actions/runs/30629207747/job/91151657388),
  `rust-integration` [91151657399](https://github.com/anhnth24/project-example/actions/runs/30629207747/job/91151657399)
  (not path-filter skip / soft pass). Integration log executed `orgs` binary tests
  including `create_org_succeeds_and_caller_becomes_owner` and
  `list_orgs_shows_only_the_callers_own_orgs`; job summary reports no ignored tests
  for that binary. Validated-context + lifecycle already landed: membership
  re-verify, fail-closed OrgContext, RLS, `GET /orgs` / `GET /orgs/{id}` /
  `POST /orgs/switch` / `POST /orgs` with owner provision + audit.

- **Plan/files:** Org create/list/detail/switch, service/repo/middleware; issue new
  context/session after verified membership.
- **Depends:** Phase 1B auth/schema. **Acceptance/tests:** Chỉ thấy org của mình;
  forged/stale header deny; two-org resolver/integration tests — green on CI
  `a62850422dd070e7e1195bfe1d4f1dee0d73566d` run
  [30629207747](https://github.com/anhnth24/project-example/actions/runs/30629207747)
  (`rust`/`web`/`rust-integration`).
- **Security/migration:** Không global org state; audit switch. **Out:** billing/OIDC.

## 1C-02 — Membership, invites và last-owner invariant

- **Status:** Done — CI exact-SHA evidence on `a62850422dd070e7e1195bfe1d4f1dee0d73566d`
  (run [30629207747](https://github.com/anhnth24/project-example/actions/runs/30629207747)):
  `rust` [91151657403](https://github.com/anhnth24/project-example/actions/runs/30629207747/job/91151657403),
  `web` [91151657388](https://github.com/anhnth24/project-example/actions/runs/30629207747/job/91151657388),
  `rust-integration` [91151657399](https://github.com/anhnth24/project-example/actions/runs/30629207747/job/91151657399)
  (not path-filter skip / soft pass). Integration log executed `members` binary tests
  including `concurrent_last_owner_race_exactly_one_survives`,
  `cross_org_denial_covers_every_member_endpoint`, and
  `member_manage_permission_required_for_patch_and_delete`; job summary reports no
  ignored tests for that binary. Implementation landed in #317+: hashed single-use
  invites, PATCH/DELETE members, transactional last-owner + owner-tier guards,
  suspend/reactivate, session family revoke, audit allowlist. Membership ACL
  `version` remains deferred to 1C-05; automated email delivery remains out of scope.

- **Plan/files:** Hashed single-use invite; membership state; transactional last-owner;
  membership version (deferred to 1C-05); session revoke. MVP chưa có mail dùng invite
  URL/token hiển thị đúng một lần cho admin copy qua kênh được tổ chức phê duyệt;
  expiry/revoke/audit bắt buộc — **đã landed**.
- **Depends:** 1C-01. **Acceptance/tests:** Không remove/downgrade last owner (kể cả tự
  thao tác chính mình); admin không quản owner; concurrent owner removal, invite
  replay/expiry, escalation tests — green on CI
  `a62850422dd070e7e1195bfe1d4f1dee0d73566d` run
  [30629207747](https://github.com/anhnth24/project-example/actions/runs/30629207747)
  (`tests/members.rs` in `rust-integration`).
- **Security/migration:** Row lock (`FOR UPDATE` trên owner rows), expand/backfill
  version (deferred to 1C-05); plaintext invite không lưu DB/log (chỉ trả 1 lần trong
  response). **Out:** automated email delivery/SCIM/MFA.

## 1C-03 — Canonical RBAC seed

- **Status:** Done — CI exact-SHA evidence on `a62850422dd070e7e1195bfe1d4f1dee0d73566d`
  (run [30629207747](https://github.com/anhnth24/project-example/actions/runs/30629207747)):
  `rust` [91151657403](https://github.com/anhnth24/project-example/actions/runs/30629207747/job/91151657403),
  `web` [91151657388](https://github.com/anhnth24/project-example/actions/runs/30629207747/job/91151657388),
  `rust-integration` [91151657399](https://github.com/anhnth24/project-example/actions/runs/30629207747/job/91151657399)
  (not path-filter skip / soft pass). Integration log executed `role_catalog` tests
  including `permissions_table_contains_exactly_active_catalog_keys ... ok` and
  `canonical_matrix_matches_builtin_role_catalog_fixture ... ok`; job summary reports
  no ignored tests for that binary. Canonical fixture
  `crates/server/openapi/builtin-role-catalog.json` is the sole built-in
  active/reserved matrix; OpenAPI references it (no embedded grants); web imports it
  for role order; DB parity tests compare exact active keys/grants. Historical
  migration `0030` catalog + per-org provision unchanged. P1C.2 disposition: matrix
  follows the fixture. Guard inventory for active operations remains 1C-04 / later PRs.

- **Plan/files:** Permission constants + DB seed owner/admin/editor/viewer; immutable
  system roles; OpenAPI/web fixture consumers.
- **Depends:** Phase 1B role schema. **Acceptance/tests:** Matrix đúng/idempotent,
  duplicate/missing/immutable mutation tests; UI không hard-code matrix — green on CI
  `a62850422dd070e7e1195bfe1d4f1dee0d73566d` run
  [30629207747](https://github.com/anhnth24/project-example/actions/runs/30629207747)
  (`role_catalog` in `rust-integration`).
- **Security/migration:** Stable keys, expand/backfill. **Out:** custom role builder.

## 1C-04 — Route/service guards và service identities

- **Status:** Done — CI exact-SHA evidence on `6833f57d94949c75ea36609e1055a1139e097c8a`
  (run [30678318560](https://github.com/anhnth24/project-example/actions/runs/30678318560)):
  changes/static
  [91310040882](https://github.com/anhnth24/project-example/actions/runs/30678318560/job/91310040882),
  `rust`
  [91310110938](https://github.com/anhnth24/project-example/actions/runs/30678318560/job/91310110938),
  `rust-integration`
  [91310110925](https://github.com/anhnth24/project-example/actions/runs/30678318560/job/91310110925)
  (not path-filter skip / soft pass). Task 7–9 reviews Approved. Integration log
  executed `tests/direct_service_authz.rs` (**6 passed; 0 failed**) covering
  `doc.delete` / `doc.publish` / `member.manage` / `audit.view` / `jobs.system`
  direct-service denials; `tests/members.rs` (**13 passed; 0 failed**); and
  `fileconv_worker` unit suite (**9 passed; 0 failed**) including all
  `worker_permissions_tests::*`. `rust` job ran
  `auth::guard_inventory` completeness/invariants green (60-row OpenAPI/route
  inventory) plus worker config fail-closed tests. Dual-layer route+service
  authorize and least-privilege worker identities landed in PR 3.


- **Plan/files:** Deny-by-default `authorize`; apply route+service+worker/reconcile;
  least-privilege identities.
- **Depends:** 1C-01/03. **Acceptance/tests:** Allow/deny mỗi permission cả hai layer;
  missing-guard inventory, direct-service và worker misuse tests.
- **Security/migration:** Không `internal=true` bypass. **Out:** generic ABAC.

## 1C-05 — Collection ACL resolver/cache

- **Status:** Done — CI exact-SHA evidence on `90742281e51d3c8ca8a32a78077a07fe3449bc68`
  (run [30649044974](https://github.com/anhnth24/project-example/actions/runs/30649044974)):
  changes/static
  [91217513655](https://github.com/anhnth24/project-example/actions/runs/30649044974/job/91217513655),
  `rust`
  [91217686329](https://github.com/anhnth24/project-example/actions/runs/30649044974/job/91217686329),
  `rust-integration`
  [91217686352](https://github.com/anhnth24/project-example/actions/runs/30649044974/job/91217686352)
  (not path-filter skip / soft pass). Task 4 review Approved; Task 5 review Approved.
  Integration log executed ACL resolver/cache targets including
  `groups_visibility_group_grant_allows_member_without_user_grant`,
  `private_visibility_ignores_group_and_role_grants`,
  `containment_removes_group_role_grants_but_preserves_other_user_grants`,
  `resolver_matches_sql_predicate_for_acl_fixture_matrix`,
  `read_grant_does_not_satisfy_write_or_admin`,
  `group_membership_revoke_invalidates_cached_context`, concurrent grant-vs-flip, and
  ACL-version bump. Canonical `(qa.query, read)` resolver projection via
  `allowed_collections_sql` with `private`/`org`/`groups` visibility + group/role grant
  branches; migration `0036` dormant-grant rejection; org-wide `acl_version` cache
  invalidation (migrations `0031`/`0033`); `auth::context_cache` freshness check on
  extractor hits.

- **Plan/files:** Private/org/groups grants (**done**); ACL/version
  snapshot (**done**: `orgs.acl_version`, migration `0031`); cache key org/user/membership/
  ACL version (**done**: `auth::context_cache::OrgContextCache`, key `(org_id, user_id)` +
  version check); invalidation APIs (**không làm riêng** — invalidation là version-bump
  trong transaction mutation, không phải API endpoint mới; không có yêu cầu nào đòi một API
  invalidation tách biệt).
- **Depends:** 1C-02/03. **Acceptance/tests:** Semantics đúng, empty/error fail closed;
  grants/status/cache/revoke tests — green on CI
  `90742281e51d3c8ca8a32a78077a07fe3449bc68` run
  [30649044974](https://github.com/anhnth24/project-example/actions/runs/30649044974)
  (`acl_resolver`, `acl_equivalence`, `acl_cache` in `rust-integration`).
- **Security/migration:** Backfill ACL version (**done**: `DEFAULT 1`, expand-only,
  migration `0031`). **Gap version-bump đã ĐÓNG (migration `0033`, 2026-07-29)**: trigger
  DB `bump_org_acl_version()` trên `collections`/`collection_user_access`/
  `org_memberships`/`roles`/`role_permissions` — bump cùng transaction cho MỌI writer,
  kể cả SQL trực tiếp (fixtures/vận hành; CI `rust-integration` bắt được đúng lỗ này
  ở `api_http_contracts`/`citation_authz_matrix` trước khi vá). **Out:**
  nested/time-based groups; operator-configurable cache capacity/TTL qua env; collection
  insert/soft-delete version bump outside `acl_mutate` (accepted TTL-bound gap).

## 1C-06 — PostgreSQL ACL enforcement

- **Status:** Done — CI exact-SHA evidence on `90742281e51d3c8ca8a32a78077a07fe3449bc68`
  (run [30649044974](https://github.com/anhnth24/project-example/actions/runs/30649044974)):
  changes/static
  [91217513655](https://github.com/anhnth24/project-example/actions/runs/30649044974/job/91217513655),
  `rust`
  [91217686329](https://github.com/anhnth24/project-example/actions/runs/30649044974/job/91217686329),
  `rust-integration`
  [91217686352](https://github.com/anhnth24/project-example/actions/runs/30649044974/job/91217686352)
  (not path-filter skip / soft pass). Task 6 rereview Approved. Integration log executed
  PostgreSQL ACL enforcement targets including
  `fts_rank_accent_fold_and_active_generation_gates` (dual `qa.query` + `qa.history`
  hydration recheck), `fts_candidate_leg_and_hydration_deny_acl_and_suspended_membership`,
  upload regression quartet (`http_upload_happy_and_spoof`,
  `cancelled_http_upload_settles_quota_consistently`,
  `envelope_binds_collection_and_stable_replay_deep_equality`,
  `quarantined_review_requires_approval_for_single_job`), private grant rejection, and
  role-grant leave-groups. Shared `db/acl_sql` predicates on FTS/hydration/conflict paths;
  explicit resolver↔SQL equivalence oracle; upload/quarantine operation-scoped write guards
  (`doc.upload` / `doc.quarantine.review` at `AccessLevel::Write`, no read-projection
  inference).

- **Plan/files:** Tenant+ACL predicates cho FTS/hydration/conflict (**done**); upload write
  gate (**done**). Autocomplete không xây (out of scope — xem điểm 4 legacy note bên dưới).
- **Depends:** 1C-05. **Acceptance/tests:** Không path thiếu context; no existence/count
  leak; SQL join/subquery/missing-predicate tests — green on CI
  `90742281e51d3c8ca8a32a78077a07fe3449bc68` run
  [30649044974](https://github.com/anhnth24/project-example/actions/runs/30649044974)
  (`retrieval`, `uploads`, fast unit pins in `db/search.rs` / `db/acl_sql.rs`).
- **Security/migration:** PG authority, prepared queries; migration `0036` dormant-grant
  rejection enforced at seed + runtime. **Out:** vector/object path (1C-07), autocomplete,
  broader route write inventory (PR 3).

### Legacy verification notes (pre-PR-2, retained for audit trail)

  1. **FTS candidate ACL subquery** (`db/search.rs`): defense-in-depth EXISTS on candidate
     leg; shared builder includes `acl_m.state = 'active'`.
  2. **Org-only count helpers** (`db/documents::count`, `db/chunks::count`): test-only;
     `org_only_count_helpers_stay_out_of_routes_and_services` source-scan guard.
  3. **Missing-predicate pin**: `every_chunk_scoped_query_embeds_acl_predicate` +
     `acl_predicate_sql_shape_is_pinned`.
  4. **Autocomplete**: không có endpoint — future work must use `acl_predicate_sql` from day one.

## 1C-07 — Qdrant/storage/jobs fail-closed enforcement

- **Status:** Done — CI exact-SHA evidence on `6833f57d94949c75ea36609e1055a1139e097c8a`
  (run [30678318560](https://github.com/anhnth24/project-example/actions/runs/30678318560)):
  `rust`
  [91310110938](https://github.com/anhnth24/project-example/actions/runs/30678318560/job/91310110938),
  `rust-integration`
  [91310110925](https://github.com/anhnth24/project-example/actions/runs/30678318560/job/91310110925)
  (not path-filter skip / soft pass). Integration log executed
  `tests/storage.rs` (**ok**; Qdrant/MinIO fail-closed + cross-org overwrite/
  dimension/missing-scope/object-key denials including
  `qdrant_connection_failure_fails_closed_as_transport`,
  `qdrant_unresponsive_endpoint_times_out_as_transport`,
  `cross_org_point_overwrite_rejected`,
  `same_org_different_collection_cannot_overwrite`,
  `missing_scope_rejects_without_network_side_effects`). `rust`/`rust-integration`
  also ran fast forged-payload / malformed / empty-scope unit pins under
  `storage::qdrant::tests::*` and retrieval forged-candidate deny. Signed-URL N/A
  (capability tokens). Connected cross-org denial suite remains **1C-12**.


- **Plan/files:** Mandatory org+non-empty collection filter; PG payload validation;
  authorize preview/download/export/job/SSE; abort in-flight on ACL change.
- **Depends:** 1C-05/06. **Acceptance/tests:** Missing/malformed/timeout/mismatch deny;
  Qdrant failure, forged payload, job ID, stream revoke, signed URL replay tests.
- **Security/migration:** No signed URL logs. **Out:** public sharing/CDN.

## 1C-08 — RLS và pool defense

- **Status:** In progress — **CI half complete** on `6833f57d94949c75ea36609e1055a1139e097c8a`
  (run [30678318560](https://github.com/anhnth24/project-example/actions/runs/30678318560), `rust-integration`
  [91310110925](https://github.com/anhnth24/project-example/actions/runs/30678318560/job/91310110925), `rust`
  [91310110938](https://github.com/anhnth24/project-example/actions/runs/30678318560/job/91310110938), `dev-stack`
  [91310110970](https://github.com/anhnth24/project-example/actions/runs/30678318560/job/91310110970)).
  Live suites: `tests/pool_worker_defense.rs` (**3 passed; 0 failed**) —
  `worker_org_context_preserves_exact_permissions_without_superset`,
  `contaminated_pool_connection_is_reset_on_next_checkout`,
  `worker_role_is_rls_scoped_and_least_privilege` (proves `markhand_worker`,
  non-superuser, non-BYPASSRLS, auth/ACL deny, cross-org RLS hide); bootstrap
  logged `migrator + app + worker (markhand_worker) roles`; worker permission/
  config unit tests green (`worker_permissions_tests::*`,
  `worker_database_url_fail_closed_without_explicit_dev_fallback`,
  `worker_app_db_fallback_requires_explicit_dev_compatibility_flag`,
  `worker_dedicated_url_takes_precedence_over_app_url`). Local
  `poc-isolation-smoke.sh` **static** mode PASSED (compose wires
  `MARKHAND_WORKER_DATABASE_URL` on six workers; Docker live probe skipped).
  **Deployed / qualifying multi-org POC half deferred to PR 5 / G1C** — do not
  treat `dev-stack` or static smoke as deployed qualification.


- **Plan/files:** Nếu ADR chọn: FORCE RLS, non-owner app role, transaction-local context,
  worker role, pool reset/verification.
- **Depends:** ADR + 1C-01/06. **Acceptance/tests:** No owner/BYPASSRLS; wrong/missing/
  pooled-context/worker misuse/migration tests.
- **Security/migration:** Expand policy trước force; nếu không chọn, close bằng ADR
  + repository evidence. **Out:** thay app guards bằng RLS.

## 1C-09 — Atomic quota lifecycle

- **Status:** Done — CI exact-SHA evidence on `6833f57d94949c75ea36609e1055a1139e097c8a`
  (run [30678318560](https://github.com/anhnth24/project-example/actions/runs/30678318560)):
  `rust-integration`
  [91310110925](https://github.com/anhnth24/project-example/actions/runs/30678318560/job/91310110925)
  (not path-filter skip / soft pass). Integration log executed
  `tests/quota.rs` (**16 passed; 0 failed**) including
  `concurrent_reserve_does_not_over_reserve`,
  `job_claim_enforces_and_releases_concurrent_slots`,
  `finalize_actual_commits_measured_token_usage`,
  `reconcile_repairs_counter_drift_and_orphaned_job_slots`,
  `upload_two_resource_settlement_is_atomic`, and idempotency/expiry/overflow
  paths. Embedding-provider token metering remains backlog (out of issue scope).


- **Plan/files:** Reserve/finalize/refund, idempotency/expiry/sweeper/reconcile cho
  storage/token/jobs.
- **Depends:** Phase 1B jobs + 1C-01. **Acceptance/tests:** 100 concurrent reservations
  không over-limit; crash/retry/cancel/timeout/actual-usage tests.
- **Security/migration:** Checked arithmetic, org/resource unique key. **Out:** billing.

## 1C-10 — Rate limit và per-org fairness

- **Status:** Done — CI exact-SHA evidence on `6833f57d94949c75ea36609e1055a1139e097c8a`
  (run [30678318560](https://github.com/anhnth24/project-example/actions/runs/30678318560)):
  `rust`
  [91310110938](https://github.com/anhnth24/project-example/actions/runs/30678318560/job/91310110938),
  `rust-integration`
  [91310110925](https://github.com/anhnth24/project-example/actions/runs/30678318560/job/91310110925)
  (not path-filter skip / soft pass). Integration log executed
  `tests/noisy_neighbor.rs` (**3 passed; 0 failed**) —
  `noisy_org_backlog_does_not_starve_quiet_org`,
  `slot_exhausted_noisy_org_falls_through_to_quiet_org_same_cycle`, plus shared
  worker-context pin. Fast unit coverage for per-org rate bucket and
  `workers::fairness::OrgRotation` green in `rust` job. Fairness SLO wall-clock
  remains **1C-13**; GPU semaphore remains N/A-until-GPU.


- **Plan/files:** User/IP/auth limits (**done từ trước**); per-org API bucket (**done**:
  `middleware/rate_limit.rs::check_org` + `routes/rate_limit_guard.rs`); per-org worker
  fairness (**done**: `workers/fairness.rs::OrgRotation` + `bin/worker.rs` multi-org
  env); GPU scheduler/semaphore (**N/A-until-GPU**, xem điều kiện kích hoạt ở trên);
  headers, privacy-safe metrics (**done từ trước**, scope mới `org` trong 429 details
  là giá trị tĩnh, không PII).
- **Depends:** 1C-09 (dùng lại reservation `concurrent_jobs`) + Phase 0 SLO/capacity
  (chỉ còn cần cho phần SLO 1C-13). **Acceptance/tests:** Noisy org không bỏ đói org
  khác — **done ở mức deterministic-count** (`tests/noisy_neighbor.rs` DB-gated + unit
  fairness/rate-limit fast); burst/window/crash-release/proxy tests đã có từ trước
  (`rate_limit.rs` tests, `sse_stream_readiness.rs` trusted-proxy 429); riêng
  **fairness SLO wall-clock** chuyển 1C-13.
- **Security/migration:** Chỉ trusted proxy IP, bounded state (org bucket nằm trong
  cùng HashMap HARD_CAP 10k key, evictable như các key khác). Không migration mới.
  **Out:** multi-region; per-org tiered rate limit từ `org_quotas` (đợi yêu cầu tier
  thật); GPU semaphore (until-GPU).

## 1C-11 — Audit/admin APIs

- **Status:** Done — CI exact-SHA evidence on `6833f57d94949c75ea36609e1055a1139e097c8a`
  (run [30678318560](https://github.com/anhnth24/project-example/actions/runs/30678318560)):
  `rust-integration`
  [91310110925](https://github.com/anhnth24/project-example/actions/runs/30678318560/job/91310110925)
  (not path-filter skip / soft pass) for audit coverage, **plus** owner-approved
  accepted risk `AR-1C-AUDIT-RETENTION` recorded in PR 1 / P1C.6 (POC/
  non-production only; Phase 4 owns retention/TTL/tamper/export — **retention is
  not implemented here**). Integration log executed `tests/audit_read.rs`
  (**9 passed; 0 failed**) including pagination, action/actor/time filters,
  `audit.view` 403 + deny-audit row, cross-org isolation, and metadata allowlist
  redaction. Direct-service `audit_view_permission_required_at_direct_list_page`
  green in `tests/direct_service_authz.rs`.


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
