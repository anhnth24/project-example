# Phase 1C issues — Multi-org security

Parent plan: [`../../../phase-1c-multi-org-security.md`](../../../phase-1c-multi-org-security.md)

<!-- roadmap-default-status: backlog -->

**Audit 2026-07-31 (PR 2 ACL enforcement).** Substrate 1B vẫn là nền; PR 1 đóng org
lifecycle / membership / canonical RBAC catalog; PR 2 đóng collection ACL resolver/cache
và PostgreSQL enforcement với CI exact-SHA evidence. `AR-1C-AUDIT-RETENTION` giữ
POC/non-production only.

- **5 Done** — **1C-01**, **1C-02**, **1C-03**, **1C-05**, **1C-06** (CI `rust` +
  `rust-integration` trên `90742281e51d3c8ca8a32a78077a07fe3449bc68`, run
  [30649044974](https://github.com/anhnth24/project-example/actions/runs/30649044974)).
- **7 In progress** — 1C-04, 1C-07…1C-12 (route guards, Qdrant/RLS/quota/denial suite còn mở).
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

- **Status:** In progress — deny-by-default `require_permission` (`auth/permissions.rs:118`) áp ở **cả route lẫn service** (nhiều endpoint), DB role least-priv migrator/app (`migrations/0027/0028`), test unit + DB-gated (`citation_authz_matrix.rs:537`). **Thiếu**: guard ở tầng worker/reconcile (worker chạy với **permission rỗng** `bin/worker.rs:153`, chỉ có tenant scope), không có missing-guard inventory (ROUTE_INVENTORY chỉ là parity method/path), chưa phủ allow/deny cho doc.delete/publish/member.manage/audit.view/jobs.system.

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

- **Status:** In progress — Qdrant mandatory org+collection filter (`storage/qdrant.rs:897`), PG payload validation, download capability authorize+replay (`services/download.rs`), authorize preview/download/export/job/SSE, abort in-flight → `citation_revoked` (`services/qa/ask_stream.rs`); test tốt (fast + DB/MinIO/Qdrant-gated). **Gap "vài fast-unit deny (forged/timeout Qdrant client)" đã ĐÓNG (2026-07-30, xác minh bằng đọc code trước khi viết — gap là thật, không stale)**: trước đó forged-payload chỉ có test Qdrant-gated (`cross_org_point_overwrite_rejected`/`same_org_different_collection_cannot_overwrite`, `tests/storage.rs`) và timeout/failure của client hoàn toàn CHƯA có test. Đã thêm fast-unit (chạy trong `cargo test` mặc định, không cần service): (a) forged payload phía response — hit/scroll page mang org/collection ngoài scope → `OwnershipConflict` (`storage::qdrant::tests::{forged_search_hit_payload_denies_with_ownership_conflict, forged_scrolled_point_payload_denies_with_ownership_conflict}`) + defense-in-depth tầng service (`services::retrieval::vector::tests::forged_candidate_collection_denies_with_ownership_conflict`); (b) forged payload phía upsert deny client-side TRƯỚC mọi network I/O (`storage::qdrant::tests::upsert_denies_forged_payload_scope_before_any_network`); (c) malformed payload/point-id từ Qdrant → lỗi, không default-fill (`malformed_qdrant_payload_fails_closed`), nil-collection lẩn trong scope set → `MissingScope` (`scope_with_nil_collection_member_is_rejected`); (d) client failure/timeout fail-closed thành `StorageError::Transport` — connection refused cho search/get_points/upsert/delete và endpoint nhận TCP nhưng không trả lời → timeout bounded (`tests/storage.rs::{qdrant_connection_failure_fails_closed_as_transport, qdrant_unresponsive_endpoint_times_out_as_transport}`, không `#[ignore]`). Ở tầng retrieval, leg Qdrant lỗi/timeout chỉ degrade thành FTS-only CÓ warning (cả hai leg lỗi → `BothLegsFailed`), không bao giờ degrade thành query không filter — filter gắn trong client trước khi gửi request, không nhánh lỗi nào bỏ filter (`services/retrieval/mod.rs:411`). Các acceptance item còn lại xác minh ĐÃ có test từ trước: missing scope (`empty_scope_is_rejected` + `missing_scope_rejects_without_network_side_effects`), malformed/tamper/expiry capability (unit trong `services/download.rs`), replay + concurrent redemption (`citation_authz_matrix.rs::live_citation_authz_expiry_replay_idor_and_immediate_deny`), job ID/SSE IDOR (`sse_stream_readiness.rs::live_job_sse_replay_worker_restart_and_cross_org_idor`), stream revoke (`ask_grounding_matrix.rs` `citation_revoked` + `live_ask_stream_jwt_exp_membership_and_delete_barriers`), dimension mismatch (`existing_collection_dimension_mismatch_rejected`, Qdrant-gated). Signed-URL không áp dụng — thay bằng capability token (theo thiết kế); phần enforcement fail-closed của 1C-07 coi như đủ test hai tầng (fast + gated), phần "suite gắn kết" cross-org còn lại thuộc 1C-12.

- **Plan/files:** Mandatory org+non-empty collection filter; PG payload validation;
  authorize preview/download/export/job/SSE; abort in-flight on ACL change.
- **Depends:** 1C-05/06. **Acceptance/tests:** Missing/malformed/timeout/mismatch deny;
  Qdrant failure, forged payload, job ID, stream revoke, signed URL replay tests.
- **Security/migration:** No signed URL logs. **Out:** public sharing/CDN.

## 1C-08 — RLS và pool defense

- **Status:** In progress — FORCE RLS ~26 bảng (`migrations/0010`), app role không owner/không BYPASSRLS (`0027/0028`), GUC transaction-local `set_config('app.org_id',…,true)` (`db/pool.rs::apply_org_context`), FORCE-RLS pin bằng unit test trên nguồn migration (`database.rs` tests — ghi chú: dòng status cũ ghi "assert FORCE-RLS ở startup, database.rs:309" là KHÔNG chính xác/stale: vị trí đó là unit test trên source migration, không có probe runtime nào ở startup assert `relforcerowsecurity`; sửa lại cho đúng hiện trạng), test pool-leak/cross-org/force-rls/deploy-role (DB-gated). **Hai gap "Thiếu" đã ĐÓNG (2026-07-30, xác minh code trước khi sửa — cả hai gap là thật):** (a) *pool reset lúc checkout*: pool trước đó dùng deadpool `RecyclingMethod::Fast` mặc định (không cleanup gì khi tái sử dụng connection). GUC transaction-local tự hết sau COMMIT/ROLLBACK by-construction (test sẵn `repositories.rs::pool_does_not_leak_tenant_gucs`), nhưng misuse **session-level** (`set_config(...,false)`, advisory lock rò, `LISTEN`, temp table) trên đường `pool.get()` thô (readiness probe, write-gate, code tương lai) sống qua checkin/checkout. Đã chuyển sang `RecyclingMethod::Clean` trong `db/pool.rs::create_pool_with_max_size` (áp dụng cho mọi pool: API, write-gate, worker): `CLOSE ALL; SET SESSION AUTHORIZATION DEFAULT; RESET ALL; UNLISTEN *; pg_advisory_unlock_all(); DISCARD TEMP; DISCARD SEQUENCES` mỗi lần recycle. Chi phí: 1 round-trip batched cho mỗi checkout connection tái sử dụng — đo cục bộ ~75µs/checkout (localhost, gồm cả recycle query, 200 vòng); KHÔNG mất prepared-statement cache (Clean cố ý không `DEALLOCATE ALL`/`DISCARD PLAN`); write-gate không xung đột vì shared advisory lock đã release trước khi client về pool (unlock_all lúc recycle chỉ là backstop). Test DB-gated mới `tests/pool_worker_defense.rs::contaminated_pool_connection_is_reset_on_next_checkout`: pool max_size=1, nhiễm session GUC + advisory lock → checkout sau phải sạch (`current_setting`/`markhand_current_org_id()` NULL, 0 advisory lock); đã chạy negative control — test FAIL đúng như kỳ vọng khi tạm quay lại `Fast`. (b) *worker role riêng*: migration mới `0035_expand_worker_role.sql` — grant guarded `IF EXISTS markhand_worker` (role LOGIN do deploy/ops provision, theo đúng tiền lệ `markhand_app`/0027: password không bao giờ nằm trong migration): DML chỉ trên bảng worker binary thật sự chạm (jobs/outbox_events/event_log; documents/document_versions/derived_artifacts/chunks/claims; index_metadata/index_generation_backfills/embedding_batches/vector_cleanup_intents; quota_reservations/usage_counters + org_quotas read-only), audit_log append-only (SELECT+INSERT, REVOKE UPDATE/DELETE/TRUNCATE — giống app), KHÔNG một grant nào trên auth/session/membership/ACL/invite/chat/upload/capability (unit test pin shape + negative list: `database.rs::worker_role_grants_are_guarded_scoped_and_append_only_audit`). Worker connect: env mới `MARKHAND_WORKER_DATABASE_URL` (hoặc `workerDatabaseUrl` trong config file), **fallback về `MARKHAND_DATABASE_URL`** khi vắng — fail-open-config CÓ CHỦ ĐÍCH để deploy cũ (chưa provision role) không gãy, cùng triết lý 0027 coi role-provisioning là việc ops; trade-off ghi rõ trong doc comment `config.rs` (quên set biến ⇒ worker âm thầm chạy bằng app role rộng hơn; RLS/FORCE RLS vẫn bó blast radius, và test posture bắt được khi role dùng thật); unit test `config.rs::worker_role_prefers_dedicated_database_url_with_app_fallback` (worker ưu tiên biến mới, fallback đúng, API role không bao giờ nhặt biến worker). Test DB-gated `tests/pool_worker_defense.rs::worker_role_is_rls_scoped_and_least_privilege`: worker không superuser/không BYPASSRLS/không owner bảng RLS, enqueue+claim job chạy được trong org của mình, org khác claim rỗng + count 0 (RLS), không context ⇒ 0 hàng (FORCE RLS chứ không phải grant), permission-denied 42501 trên refresh_tokens/org_memberships/org_invites/collection_user_access/qa_chat_sessions/upload_operations, không UPDATE audit_log, không CREATE TABLE. **Còn lại (ops, ngoài phạm vi code vòng này):** deploy/compose thật chưa provision role `markhand_worker` + chưa set `MARKHAND_WORKER_DATABASE_URL` — migration idempotent + fallback đã sẵn sàng, bật lúc nào cũng được.

- **Plan/files:** Nếu ADR chọn: FORCE RLS, non-owner app role, transaction-local context,
  worker role, pool reset/verification.
- **Depends:** ADR + 1C-01/06. **Acceptance/tests:** No owner/BYPASSRLS; wrong/missing/
  pooled-context/worker misuse/migration tests.
- **Security/migration:** Expand policy trước force; nếu không chọn, close bằng ADR
  + repository evidence. **Out:** thay app guards bằng RLS.

## 1C-09 — Atomic quota lifecycle

- **Status:** In progress — reserve/finalize/refund + reserve_upload hai-tài-nguyên atomic (`services/quota.rs`), idempotency theo key, expiry, sweeper **đã wire** vào background (`http.rs`), checked arithmetic, advisory lock. Đợt này đóng 4 phần "Thiếu" cũ (đã xác minh từng điểm trước khi làm):
  (a) **Token-quota lifecycle**: consumer token thật duy nhất trên đường `ask` là **chat provider** (`ChatProvider::complete`/`stream_tokens` — được gọi cả ở chế độ fail-closed để đo outage); khi không cấu hình provider (MVP extractive-only) không tiêu token nên không reserve. Reserve ước lượng (prompt + `MAX_ANSWER_CHARS`/4, heuristic ~4 chars/token vì response OpenAI-compat/GLM stream không bảo đảm block `usage`) trước khi gọi provider ở cả `ask()` JSON (`services/qa/mod.rs`) lẫn `ask/stream` (`ask_stream.rs::start_ask_stream`, reserve TRƯỚC khi tạo durable session → deny là 429 sạch không side-effect); settle usage thật qua `quota::finalize_actual` mới (commit số đo, không phải số ước lượng); refund khi transport fail, commit prompt-only khi timeout, commit phần đã stream khi cancel/`citation_revoked` giữa chừng (token provider đã tiêu thì không refund). Hết quota → 429 `quota_exceeded` + `x-quota-*` headers (tái dùng nguyên `QuotaError` contract của upload, `routes/ask.rs`) + audit `quota.deny`. Token của embedding-provider (index/backfill hàng loạt) **chưa** meter — backlog riêng, gắn với job lifecycle chứ không phải request `ask`.
  (b) **Concurrent-jobs enforce ở prod**: mọi đường claim prod (`jobs::claim`/`claim_type`/`claim_reconcile` — chính là đường `bin/worker.rs` → `workers/*::run_once`) giờ lấy advisory lock `concurrent_jobs`, clamp limit theo slot còn trống, insert reservation `job.slot.{job_id}.{uuid}` (amount 1, TTL = lease TTL) **atomic trong cùng txn claim**; heartbeat gia hạn reservation cùng lease; complete/fail/cancel(+children)/reclaim/dry-run-release refund slot ngay. Hết slot → claim trả rỗng (worker idle-poll); org thiếu `org_quotas` → fail-closed lỗi cấu hình rõ ràng (cùng posture upload).
  (c) **Quota reconcile**: task nền `http.rs::start_quota_reconcile` (knob `MARKHAND_QUOTA_RECONCILE_INTERVAL_SECS`, default 3600s, min 60s, `0` = off; cùng pattern maintenance-lock + ops-fence guard như sweeper) gọi `quota::reconcile_all_orgs`: đối chiếu `usage_counters` với ground truth (storage = bytes version + derived artifacts của documents còn sống; documents = count sống) và refund slot `concurrent_jobs` mồ côi (job không còn leased). Drift → upsert counter + audit `quota.reconcile` (action mới, allowlist chỉ số liệu, actor NULL qua `audit::record_system_in_txn` vì là system action). **Lưu ý ngữ nghĩa**: trước đây counter storage/documents là cộng dồn vĩnh viễn (xoá tài liệu không trả quota); reconcile đưa counter về mức sử dụng thật → sau purge quota được giải phóng ở lần reconcile kế. Tokens **không** reconcile (tiêu ở provider ngoài, không có ground truth đếm lại được).
  (d) **Test concurrent ≥100**: `tests/quota.rs::concurrent_reserve_does_not_over_reserve` nâng 16 → **100 task thật** (pool cố định 16, deadpool xếp hàng không timeout + advisory lock serialize admission → không flaky). Test DB-gated mới: claim bị clamp theo `max_concurrent_jobs` + release đủ đường complete/fail/reclaim; reconcile sửa drift + refund slot mồ côi + audit row (actor NULL, idempotent lần 2); `finalize_actual` commit số đo/refund khi 0/idempotency terminal. Suite quota 14/14, jobs 18/18, pool_worker_defense 2/2 xanh cục bộ (PG16).
  **Còn lại (out of đợt này)**: meter token embedding-provider theo job; billing (đã Out từ đầu).

- **Plan/files:** Reserve/finalize/refund, idempotency/expiry/sweeper/reconcile cho
  storage/token/jobs.
- **Depends:** Phase 1B jobs + 1C-01. **Acceptance/tests:** 100 concurrent reservations
  không over-limit; crash/retry/cancel/timeout/actual-usage tests.
- **Security/migration:** Checked arithmetic, org/resource unique key. **Out:** billing.

## 1C-10 — Rate limit và per-org fairness

- **Status:** In progress — limiter per-IP/user/route (`middleware/rate_limit.rs`) +
  Retry-After header + metrics privacy-safe + worker type-fairness
  (`workers/index.rs:137`) đã có từ trước. **Phần per-ORG fairness đã landed
  (2026-07-30, xác minh từng điểm "Thiếu" bằng code trước khi xây):**
  1. **Bucket rate-limit per-ORG thật** (gap cũ đúng: key `user:{org}:{user}` chỉ dùng
     org làm prefix — mỗi user vẫn có bucket riêng, org N user = N× capacity): thêm tầng
     thứ 3 `RateLimiter::check_org` (key `org:{org_id}`, `org_per_minute` default 600,
     env `MARKHAND_RATE_ORG_PER_MINUTE`, compose POC set 3000). Wire tại MỘT điểm
     `routes/rate_limit_guard.rs::check_user` (user bucket trước → scope `user` cho user
     tự vượt; org bucket sau → scope `org`) nên mọi route authenticated
     (ask/search/upload/events/reindex) tự có tầng org. **Nguồn limit chọn env knob
     đồng nhất, KHÔNG cột `org_quotas`** — trade-off ghi rõ: limiter sync in-process
     (không DB trên request path), theo tiền lệ `MARKHAND_RATE_*`; per-org tiered limit
     (cột `org_quotas` + cached read) để lại đến khi có yêu cầu tier thật. Test unit
     fast `org_bucket_bounds_many_users_without_touching_other_orgs` (org A 10 user chỉ
     lọt đúng org-capacity, org B nguyên vẹn).
  2. **Worker fairness đa-org** — xác minh "1 org/tiến trình" nghĩa thật: worker pin org
     qua env `MARKHAND_WORKER_ORG_ID` (một UUID, `bin/worker.rs:151` cũ), mọi claim txn
     set GUC RLS org đó; claim SQL CÓ lọc org (`org_id = $1` + FORCE RLS) nên "fair
     ORDER BY xuyên org trong query claim" là bất khả thi nếu không phá posture RLS
     (worker không context thấy 0 hàng — `pool_worker_defense`). Giải pháp trong hạ tầng
     hiện có: `workers/fairness.rs::OrgRotation` — `MARKHAND_WORKER_ORG_ID` nhận danh
     sách UUID phẩy, mỗi cycle round-robin quét từ cursor, phục vụ tối đa 1 job rồi đẩy
     cursor qua org vừa phục vụ ⇒ **bound xác định: giữa 2 job liên tiếp của một org có
     backlog, tối đa N-1 job org khác chen vào**, bất kể backlog org ồn to bao nhiêu.
     Poison-org (attempt lỗi) cũng bị đẩy cursor qua để không ghim đầu rotation. Tái
     dụng nguyên claim path 1C-09 (advisory lock + reservation `concurrent_jobs` clamp
     trong claim txn) — không xây scheduler trùng. Đơn-org giữ nguyên hành vi cũ
     (rotation 1 phần tử); reconcile oneshot bắt buộc đúng 1 org.
  3. **Test noisy-neighbor DB-gated** `tests/noisy_neighbor.rs` (đo bằng ĐẾM thứ tự
     claim, không wall-clock — SLO wall-clock thuộc 1C-13): (a) org A 20 job vs org B
     4 job → chuỗi phục vụ xen kẽ đúng `[A,B,A,B,A,B,A,B]` rồi A-only, đủ 24; (b) org A
     giữ chặt slot `concurrent_jobs` duy nhất (lease không complete) + còn backlog →
     admission 1C-09 trả claim rỗng và rotation rơi xuống org B **trong cùng một
     cycle**; cả hai org cạn/kẹt → cycle idle (không busy-loop). Unit fast trong
     `workers/fairness.rs` phủ alternation/skip-idle/poison-advance/duplicate-reject.
  **GPU scheduler/semaphore per-org: N/A-until-GPU (xác minh 2026-07-30, không xây)** —
  grep toàn repo: server crate KHÔNG có GPU workload nào; GPU chỉ xuất hiện ở (a)
  feature `cuda` opt-in của whisper trong `crates/core` (desktop/CLI, ngoài server),
  (b) container vLLM/embedding GPU opt-in profile trong `deploy/compose.spike.yml`/
  `deploy/dev/compose.yml` (external HTTP provider, POC default `mock`). **Điều kiện
  kích hoạt**: khi một GPU inference service dùng chung (vLLM/TEI nội bộ) vào đường
  serving thật (embedding profile ≠ mock trỏ GPU chung), cần per-org admission tại
  call-site embedding/ask (semaphore keyed org) — job-level đã được `concurrent_jobs`
  bound sẵn. **SLO wall-clock test**: thuộc 1C-13 (cần Phase 0 capacity baseline,
  chưa có) — không làm ở đây.

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
