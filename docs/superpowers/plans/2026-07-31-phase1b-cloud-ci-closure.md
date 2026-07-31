# Phase 1B Cloud/CI Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close every remaining Phase 1B acceptance gap that can be implemented and evidenced by Cursor Cloud plus the existing GitHub `rust-integration` job, leaving only the explicitly live Docker-host gate for the owner.

**Architecture:** Extend the existing DB/MinIO/Qdrant-gated integration tests rather than add production endpoints or alternate test infrastructure. Reuse the real HTTP router and worker pipelines so authorization assertions exercise production boundaries. Keep Docker-only R06 hanging-soak execution outside this branch; its existing harness remains the owner handoff.

**Tech Stack:** Rust 1.88, axum integration tests, PostgreSQL 16, MinIO, Qdrant, GitHub Actions, Cargo.

## Global Constraints

- `fileconv-core` and `crates/knowledge` remain the only shared conversion/RAG implementations; do not duplicate business logic in tests.
- Every business read remains tenant scoped and fail closed; no test-only bypass may enter production code.
- Candidate text/citations may be returned only after PostgreSQL authorization hydration.
- New behavioral coverage follows RED → GREEN → REFACTOR and records the failing test evidence.
- DB/service-backed tests remain `#[ignore]` and are executed by `rust-integration` with `--include-ignored`.
- Do not weaken a missing-service prerequisite into a soft pass; CI must either execute the intended live path or fail clearly.
- No new dependency is permitted unless unavoidable; any manifest change must update `Cargo.lock`.
- Before every Rust push run `cargo fmt --all -- --check`, `cargo metadata --locked --format-version 1 --no-deps`, and `python3 scripts/check-dependency-policy.py`.
- R06 live hanging-dependency soak and any 24-core performance requalification are owner-machine evidence, not claims made by this branch.

---

### Task 1: Complete P1B-R04 cross-tenant HTTP contract coverage

**Files:**
- Modify: `crates/server/tests/api_http_contracts.rs`
- Modify only if a verified contract defect exists: `crates/server/src/api/openapi.rs`
- Modify only if a verified contract defect exists: `crates/server/openapi/openapi.yaml`

**Interfaces:**
- Consumes: the production `app_router`, existing live test fixtures, `ROUTE_INVENTORY`, and current capability/citation services.
- Produces: HTTP-level regression evidence for foreign capability redemption, foreign ask-stream scope, and conflict-list scoping.

- [ ] **Step 1: Add failing HTTP assertions**

Extend the existing cross-tenant tests to assert:

```text
GET  /api/v1/downloads/{foreign_capability}   -> 404, no foreign metadata/body
POST /api/v1/ask/stream with foreign scope   -> 404, no SSE content
GET  /api/v1/conflicts as tenant A           -> 200 and excludes tenant B conflict
```

Use a capability minted through the production service or route for the foreign tenant; do not handcraft or decode the token. Assert the stable error envelope (`code`, `requestId`) where the endpoint returns JSON.

- [ ] **Step 2: Verify RED**

Run the focused tests with the integration environment available:

```bash
cargo test -p fileconv-server --test api_http_contracts -- \
  --include-ignored live_http_unauthenticated_and_cross_tenant_are_consistent
cargo test -p fileconv-server --test api_http_contracts -- \
  --include-ignored live_http_retrieval_refuses_foreign_collection_scope
```

Expected: at least one new assertion fails because the path was previously untested or exposes a contract mismatch. If all pass immediately, document that the behavior already existed and retain only assertions that establish previously absent acceptance evidence.

- [ ] **Step 3: Make only verified production fixes**

If a new assertion finds an actual defect, fix the smallest route/service/OpenAPI mismatch. If behavior is already correct, do not modify production code.

- [ ] **Step 4: Verify GREEN**

```bash
cargo test -p fileconv-server --no-fail-fast --test api_http_contracts -- --include-ignored
cargo test -p fileconv-server api::openapi -- --nocapture
```

- [ ] **Step 5: Commit**

```bash
git add crates/server/tests/api_http_contracts.rs crates/server/src/api/openapi.rs crates/server/openapi/openapi.yaml
git commit -m "test(server): close Phase 1B HTTP authorization gaps"
```

Only stage production files that actually changed.

---

### Task 2: Drive P1B-R02 history authorization from worker-produced artifacts

**Files:**
- Modify: `crates/server/tests/citation_authz_matrix.rs`
- Modify: `crates/server/tests/common/mod.rs`
- Reuse patterns from: `crates/server/tests/retrieval_vertical_slice.rs`
- Modify if required to prevent CI soft-skip: `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: real upload route, `ConvertWorker`, `IndexWorker`, test embedding provider, citation/preview/download production services, and `MinioCleanupGuard`.
- Produces: worker-created version/artifact/chunk fixtures for history ACL, IDOR, and delete-denial assertions.

- [ ] **Step 1: Write a worker-produced fixture and failing replacement assertions**

Create a reusable test fixture that:

```text
HTTP upload -> ConvertWorker -> IndexWorker -> worker-created trusted artifact/chunks
```

Then replace SQL/manual-object seeding for the R02 history/IDOR/delete section with:

```text
history: upload revision v1, upload revision v2, assert old version requires history permission
IDOR: produce a document independently in org B, assert org A cannot preview/resolve it
delete: delete/tombstone the worker-produced document, assert preview/resolve immediately deny
```

Do not insert `document_versions`, `derived_artifacts`, or `chunks` directly for these three paths.

- [ ] **Step 2: Verify RED**

```bash
cargo build -p fileconv-cli --no-default-features
cargo test -p fileconv-server --test citation_authz_matrix -- \
  --include-ignored live_citation_authz_expiry_replay_idor_and_immediate_deny
```

Expected: the new worker-backed path fails before the helper/pipeline setup is complete for the expected missing fixture behavior, not because of a typo.

- [ ] **Step 3: Implement the minimal shared test harness**

Reuse the conversion/index setup from `retrieval_vertical_slice.rs`; do not duplicate production logic. Keep test buckets and IDs unique. Ensure missing Qdrant/admin prerequisites fail the intended test clearly rather than returning early as a false green.

- [ ] **Step 4: Verify GREEN**

```bash
cargo build -p fileconv-cli --no-default-features
cargo test -p fileconv-server --no-fail-fast \
  --test citation_authz_matrix --test retrieval_vertical_slice \
  -- --include-ignored
```

- [ ] **Step 5: Commit**

```bash
git add crates/server/tests/citation_authz_matrix.rs crates/server/tests/common/mod.rs \
  crates/server/tests/retrieval_vertical_slice.rs .github/workflows/ci.yml
git commit -m "test(server): exercise citation history through workers"
```

Only stage files that actually changed.

---

### Task 3: Add repeatable MinIO cleanup-guard soak coverage

**Files:**
- Modify: `crates/server/tests/citation_authz_matrix.rs` or create a focused existing-pattern integration test binary under `crates/server/tests/`
- Modify: `crates/server/tests/common/mod.rs` only if the guard needs a reusable assertion

**Interfaces:**
- Consumes: `MinioCleanupGuard` and `MinioClient::cleanup_bucket_and_assert_gone`.
- Produces: a bounded concurrent soak that repeatedly creates objects, invokes cleanup, and proves the bucket no longer exists.

- [ ] **Step 1: Add the failing soak test**

Add an ignored test named `live_minio_cleanup_guard_soak` with bounded rounds and concurrent unique buckets. Each round must:

```text
create unique bucket -> put multiple objects -> cleanup guard -> assert bucket absent
```

The test must fail if cleanup is skipped, partially deletes, or silently ignores a remaining bucket.

- [ ] **Step 2: Verify RED**

Temporarily use a test-local negative control that leaves one object/bucket behind and run:

```bash
cargo test -p fileconv-server --test citation_authz_matrix \
  live_minio_cleanup_guard_soak -- --include-ignored --nocapture
```

Expected: failure identifies the non-empty or still-existing bucket. Remove the negative control before implementation.

- [ ] **Step 3: Implement the bounded concurrent soak**

Use existing cleanup retry behavior. Do not introduce sleeps outside the existing eventual-consistency retry helper, unbounded loops, shared bucket names, or a production API used only by tests.

- [ ] **Step 4: Verify GREEN**

```bash
cargo test -p fileconv-server --test citation_authz_matrix \
  live_minio_cleanup_guard_soak -- --include-ignored --nocapture
```

- [ ] **Step 5: Commit**

```bash
git add crates/server/tests/citation_authz_matrix.rs crates/server/tests/common/mod.rs
git commit -m "test(server): soak MinIO cleanup guard"
```

---

### Task 4: Record CI evidence and synchronize Phase 1B status

**Files:**
- Modify: `plans/markhand-web/backlog/phase-1b/issues/README.md`
- Regenerate: `plans/markhand-web/roadmap.html`
- Modify if generated by the repository workflow/source: `plans/markhand-web/backlog/github-issues.json`

**Interfaces:**
- Consumes: a green GitHub `rust-integration` run for the branch commit.
- Produces: accurate R02–R05 status text tied to test names and CI evidence; R06 remains open for owner evidence.

- [ ] **Step 1: Run local deterministic preflight**

```bash
cargo fmt --all -- --check
cargo metadata --locked --format-version 1 --no-deps
python3 scripts/check-dependency-policy.py
python3 scripts/build-roadmap.py --check
```

- [ ] **Step 2: Push and obtain GitHub CI evidence**

Push the branch and require the `rust`, `rust-integration`, and static/contract jobs applicable to server changes to pass. Treat any soft-skipped worker-backed test as a failure of evidence even if the job is green.

- [ ] **Step 3: Update statuses conservatively**

Mark R03 and R05 Done only after their existing integration binaries run green. Mark R04 Done after Task 1 coverage runs green. Mark R02 Done only after both worker-produced authorization and cleanup soak run green. Keep R06 `In progress` with an explicit owner handoff for:

```bash
MARKHAND_HANGING_SOAK=1 deploy/scripts/r06-hanging-soak.sh
```

- [ ] **Step 4: Regenerate and verify roadmap**

```bash
python3 scripts/build-roadmap.py
python3 scripts/build-roadmap.py --check
```

- [ ] **Step 5: Commit**

```bash
git add plans/markhand-web/backlog/phase-1b/issues/README.md \
  plans/markhand-web/backlog/github-issues.json \
  plans/markhand-web/roadmap.html
git commit -m "docs(web): sync Phase 1B closure evidence"
```

Only stage generated files that changed.
