# P2-20 — Real E2E foundation + auth/library/upload/actions

Created: 2026-08-06
Source issue: catalog/tracker synchronization pending via [PR #394](https://github.com/anhnth24/project-example/pull/394); canonical authority remains the Phase 2 catalog entry for P2-20 (no invented GitHub issue URL)
Catalog: [Phase 2 issue catalog — P2-20](../markhand-web/backlog/phase-2/issues/README.md#p2-20--real-e2e-foundation--authlibraryuploadactions)
Phase plan: [Phase 2 Web SPA](../markhand-web/phase-2-web-spa.md)
Status: In progress

## Objective

Deliver a deterministic, fail-closed, run-namespaced real-browser harness that exercises
foundational auth/library/upload/document-action outcomes against a **built SPA** and
**real** `fileconv-server` + PostgreSQL + Qdrant + MinIO +
convert/index/embedding/delete workers, with no fetch mock, production test route, auth
bypass, or client-derived authority.

## Context

Owner-approved design:
[`docs/superpowers/specs/2026-08-06-p2-15-full-real-e2e-design.md`](../../docs/superpowers/specs/2026-08-06-p2-15-full-real-e2e-design.md).
P2-20 is the first independently reviewable child of umbrella P2-15.

**Verified baseline in this worktree (context only — not delivery evidence):**

- `python3 deploy/scripts/test_web_e2e_real_orchestration.py` → 8 passed
- `pnpm --dir web test --run` → 50 files / 541 tests passed

**Current real harness (smoke only):**

| Area | Today | Gap for P2-20 |
|---|---|---|
| Orchestrator | `deploy/scripts/web-e2e-real.sh` builds SPA, starts server + 3 workers, supervises process death during Playwright, redacts logs fail-closed | No delete worker, run namespace, fixture setup/teardown, artifact staging/manifest, secret/content canary beyond log dump, fixture-failure fail paths |
| Hermetic tests | `deploy/scripts/test_web_e2e_real_orchestration.py` (8 cases: workers, redaction, worker-death abort, cleanup, redactor fail-closed, liveness after seed) | Missing fixture refuse/leak/cleanup, artifact validation, canary, Playwright fail→teardown evidence hooks |
| Real support | `web/e2e-real/support.ts` uses fixed seeded `admin@poc.example` / `POC Library` | Must switch to runtime credentials + run-scoped resources; remove fixed-seed authority |
| Real specs | `auth.spec.ts` login; `library.spec.ts` shell+collection; `upload.spec.ts` upload→indexed→preview | Missing logout, deep-link+login, real 401 refresh, download capability/redeem, reindex, failed retry, delete, 403/429, throttled upload progress, real 413 |
| Mock parity sources | `web/e2e/auth.spec.ts`, `library.spec.ts`, `upload.spec.ts`, `document-status-polling.spec.ts` | Real ports must hit backend; may use network delay/`continue` only — never synthesize allow |
| Playwright config | `web/playwright.config.ts` `real` project; `trace: 'on-first-retry'` | Serial workers; disable/restrict traces/screenshots so credentials/content cannot leak |
| Runtime knobs already present (no API/schema change) | `MARKHAND_MAX_UPLOAD_BYTES` (`config.rs`); `MARKHAND_RATE_ROUTE_PER_MINUTE` (`middleware/rate_limit.rs`); `MARKHAND_PROFILE` | Use **dev/CI-only** lowered upload/route limits for deterministic 413/429; create a real 401 by modifying one request's bearer value in Playwright and continuing it to the real server |
| CI wiring | `DEV_STACK_MODE=full` → `deploy/scripts/dev-stack-ci.sh` → `web-e2e-real.sh` | Keep; extend script contract only |

**Binding constraints (non-negotiable):**

1. Real stack only; no fetch mock, production test route, auth bypass, or client authority.
2. Unique run namespace; runtime-generated credentials; fixture tooling refuses `MARKHAND_PROFILE=prod`.
3. Bounded fail-closed teardown for users/sessions/org/collection/documents/uploads/jobs/objects/vectors attributable to the run.
4. Supervisor fails on required process death, Playwright failure, fixture setup/cleanup failure, redactor failure, artifact validation failure, or secret/content canary.
5. Real tests stay serial; required scenarios cannot skip.
6. Network fault shaping may delay/slow then `continue` to the real backend; must not synthesize success/authorization.
7. P2-20 owns base staging/redaction/validation; P2-23 owns release-wide retention/enforcement.
8. No dependency/pin/converter/native/public API/schema changes. If reality requires one, stop and record a **blocker** — do not implement it under this plan.
9. Avoid Rust production-code changes. Mandatory Rust pre-push gates apply only if Rust files change.

## Implementation plan

Work proceeds as independently reviewable tasks (Task 1→Task 8). Each task has its own RED/GREEN
TDD loop, negative paths, and commit boundary. Do not start production/test code until this
plan is linked and plan status moves to `In progress` at the first code change.

### Task 1 — Run-scoped fixture tooling (setup / cleanup / refuse)

**Outcome:** A dev/CI-only CLI under `deploy/scripts/` creates and tears down a unique run
namespace with runtime credentials and resource IDs; refuses production profile.

**Files / interfaces (exact):**

- Add `deploy/scripts/web_e2e_real_fixture.py` exposing CLI:
  - `setup --run-id <id> --manifest-out <manifest.json> --credentials-out <credentials.json>`
    → writes an ID/checksum-only public manifest plus a mode-`0600` runtime credential
    file; neither output is tracked
  - `cleanup --run-id <id> --manifest <manifest.json> --credentials <credentials.json>
    --api-base <url> --timeout-secs <n>` → while the server and delete worker are alive,
    authenticate through the public API, request deletion for every run-owned document,
    wait boundedly for object/vector cleanup, then remove remaining run-owned database
    rows in an explicit reviewed foreign-key order and remove the credentials file
  - `verify-clean --run-id <id> --manifest <manifest.json>` → fail if leaks remain
- The Python module must isolate subprocess/HTTP execution behind an injected runner so
  `deploy/scripts/test_web_e2e_real_fixture.py` can use deterministic fake
  `psql`/HTTP/MinIO/Qdrant results without a live stack. T8 adds a live Compose
  setup→cleanup→verify-clean cycle.
- Reuse existing seed primitives patterns from `seed-dev-password.sh` /
  `seed-poc-org.sh` / Postgres `psql` via Compose; do **not** change migrations or
  production seed SQL

**Fixture contents (minimum):**

- Unique `run_id` (UUID or `e2e-{sha}-{n}`)
- Runtime user email/password (generated; password only in masked env file under
  artifact dir / CI secrets mask)
- Run-scoped org + membership with permissions needed for happy paths
  (`doc.upload`, library read, download, reindex/delete as required by scenarios)
- One collection with stable display name keyed by run
- Secondary viewer actor with collection read visibility but no `doc.upload`; its
  reindex request must reach the real route and produce the required 403 mapping
- One document row in terminal `failed` state for retry, seeded with the existing
  `documents`, `document_versions`, and `documents.current_version_id` columns — **no
  schema change**; if live constraints reject this existing representation, stop and
  escalate the exact blocker
- Fixture checksum over sorted ID list (no secrets)

**Production refusal:** If `MARKHAND_PROFILE=prod` (or equivalent prod detection already
used by server config), CLI exits non-zero before any write.

**TDD steps:**

1. RED: add tests asserting `setup` with `MARKHAND_PROFILE=prod` exits ≠0 and writes
   nothing; credentials are mode `0600`; `cleanup`/`verify-clean` detect a planted DB,
   object, or vector leak ID; cleanup timeout is non-zero.
2. GREEN: implement refusal, runtime JSON files, bounded API/object/vector/database
   cleanup, checksum write, and idempotent `verify-clean`.
3. Negative: missing Compose/DB → fail closed; partial cleanup → non-zero + leak report
   (IDs only).

**Commands:**

```bash
python3 deploy/scripts/test_web_e2e_real_fixture.py
```

**Expected RED reason:** refuse/cleanup/leak assertions absent before implementation.

**Commit boundary:** `test(p2-20): add real E2E fixture tooling and refuse/leak tests`

---

### Task 2 — Orchestrator fail-closed extensions

**Outcome:** `web-e2e-real.sh` owns run ID, calls fixture setup before Playwright, always
attempts cleanup, stages sanitized artifacts, validates canaries, and fails the job on
every required failure class.

**Files / interfaces:**

- Extend `deploy/scripts/web-e2e-real.sh`:
  - Generate `WEB_E2E_REAL_RUN_ID` when unset
  - Export runtime env for Playwright (`MARKHAND_E2E_REAL_*` from fixture out-file)
  - Set **dev-only** lowered knobs for this process only (document in script comments):
    - `MARKHAND_MAX_UPLOAD_BYTES=4096` for deterministic 413
    - `MARKHAND_RATE_ROUTE_PER_MINUTE=1` for deterministic reindex 429
  - Why runtime config (not seams): these knobs already exist in
    `crates/server/src/config.rs` / `middleware/rate_limit.rs` and are valid ops
    overrides; they are **not** test-only routes, bypasses, or schema forks. Prod
    profile continues to refuse unsafe defaults via existing config validation.
  - Artifact dir: `WEB_E2E_REAL_ARTIFACT_DIR` (default under `/tmp` or CI workspace)
  - After Playwright: write sanitized manifest JSON; run secret/content canary scan;
    checksum artifacts; fail if missing/mismatched
  - Cleanup in `trap` **after** tests (success or fail); cleanup failure → non-zero
  - Keep existing process supervision + redactor fail-closed behavior
  - Start and supervise a `delete` worker so browser delete and teardown remove MinIO
    objects and Qdrant points before database fixture rows are removed
- Add `deploy/scripts/web_e2e_real_artifacts.py` in this task with explicit
  `write --results <playwright-json> --fixture <manifest> --out <manifest>` and
  `validate --manifest <manifest> --artifact-dir <dir>` commands. Validation requires
  the P2-20 fields/checksums and scans staged files for configured secret/content
  canaries; missing results, skipped required scenarios, checksum drift, or a canary
  match exits non-zero.
- Extend `deploy/scripts/test_web_e2e_real_orchestration.py` with hermetic cases for:
  - fixture setup failure aborts before Playwright
  - fixture cleanup failure fails the job
  - redactor failure / canary match fails
  - artifact validation failure fails
  - Playwright failure still runs cleanup
  - required process death still aborts Playwright (existing)

**Sanitized manifest must include:** scenario inventory, SHA/ref, tool versions
(Playwright/Node/pnpm as available), fixture checksum, durations/outcomes, skipped
count (must be 0 for required), teardown result, artifact checksums.

**Traces/screenshots:** the real project sets trace, screenshot, and video to `off`;
never retain credential-bearing UI dumps (P2-20 base policy; P2-23 owns any future
reviewed non-content allowlist and release retention).

**TDD steps:**

1. RED: orchestration tests for fixture/artifact/canary failure classes fail.
2. GREEN: wire hooks into `web-e2e-real.sh` with shims in the hermetic harness.
3. Negative: ensure no raw secret appears in combined stdout/stderr on failure dumps.

**Commands:**

```bash
python3 deploy/scripts/test_web_e2e_real_orchestration.py
```

**Commit boundary:** `feat(p2-20): extend real E2E orchestrator with fixtures and artifacts`

---

### Task 3 — Real Playwright support refactor + serial / artifact policy

**Outcome:** `web/e2e-real/support.ts` loads runtime credentials and run-scoped IDs;
`web/e2e-real/runtime.ts` owns pure JSON/env parsing; the real project runs serially
with traces/screenshots disabled.

**Files / interfaces:**

- Refactor `web/e2e-real/support.ts`:
  - `runtimeCredentials()` from the mode-`0600` JSON path in
    `MARKHAND_E2E_REAL_CREDENTIALS_FILE` (fail if missing — no fixed seed fallback)
  - `runtimeFixture()` from `MARKHAND_E2E_REAL_FIXTURE_FILE` for run-scoped names/IDs
  - `login(page)`, `logout(page)`
  - `openRunCollection(page)` using fixture collection name/id
  - Network shaping helper: `delayThenContinue(page, urlGlob, delayMs)` using
    `route.continue()` only
- Add `web/e2e-real/runtime.ts` with pure
  `loadRuntimeCredentials(env)` / `loadRuntimeFixture(env)` parsers and
  `web/src/test/e2eRealRuntimeConfig.test.ts` for missing path, malformed JSON,
  required-field, and fixed-seed-fallback rejection.
- Update `web/playwright.config.ts` for `REAL_MODE`:
  - `workers: 1` / `fullyParallel: false` for real project
  - `trace: 'off'`; `screenshot: 'off'`; `video: 'off'`
  - Keep mutual exclusion with mock project
- Add `web/src/test/playwrightRealConfig.test.ts`, which imports the exported config
  factory and asserts real mode has one worker, no parallelism, and all three browser
  artifacts disabled while mock mode retains its current behavior.
- Do **not** change mock `web/e2e/**` behavior except accidental shared config that must
  remain mock-parallel

**TDD steps:**

1. RED: `web/src/test/e2eRealRuntimeConfig.test.ts` fails because the parser does not
   exist; config contract test fails until real mode is serial with all retained browser
   artifacts disabled.
2. GREEN: implement helpers; keep existing three smoke specs temporarily adapted to
   runtime env so baseline does not break when stack runs.

**Commands:**

```bash
pnpm --dir web exec playwright test --list  # config loads
pnpm --dir web exec vitest run src/test/e2eRealRuntimeConfig.test.ts
```

**Commit boundary:** `refactor(p2-20): runtime-credential real E2E support and serial policy`

---

### Task 4 — Auth scenarios (login / logout / deep-link / real 401 refresh)

**Outcome:** Port mock `web/e2e/auth.spec.ts` outcomes to real backend.

**Files:**

- Expand `web/e2e-real/auth.spec.ts`

**Scenarios (required, no skip):**

1. Login with runtime credentials → shell visible
2. Logout → `/login`, no library rail
3. Anonymous deep-link to run collection path → `/login?next=` preserved → successful
   login → lands on sanitized intended route (`PublicOnlyRoute` / `sanitizeNextPath`)
4. One **real** backend 401 recovered via refresh/retry without bounce to `/login`:
   install a one-shot Playwright route for `GET /api/v1/auth/me`, replace only the first
   request's bearer value with an invalid value, and `route.continue()` to the real
   server. Observe the real 401, real `POST /auth/refresh` 200, and retried real
   `GET /auth/me` 200. Never `fulfill()` any of those responses.

**TDD:**

1. RED: add specs; run under real project (or list+compile) — fail until stack+impl ready.
2. GREEN: implement using support helpers only.

**Negative:** failed refresh must still bounce to login (assert if cheap; otherwise rely
on existing unit coverage in `api/client` and document).

**Commit boundary:** `test(p2-20): add real auth login logout deeplink refresh scenarios`

---

### Task 5 — Collection navigation + indexed preview + download capability/redeem

**Outcome:** Real collection open, preview of indexed markdown, download Markdown path
issues capability and redeems through real API/storage.

**Files:**

- Expand `web/e2e-real/library.spec.ts`

**Scenarios:**

1. Navigate to run collection; upload panel visible
2. Upload a unique tiny text document inside this scenario, wait for the real workers to
   index it, and assert preview markdown content (the content canary must ensure staged
   artifacts do not retain body text)
3. Download → Markdown: UI triggers `issueDownloadCapability` + `redeemDownload`; assert
   success notice / download completion without exposing capability token in logs

**TDD:** RED specs first; GREEN against real stack.

**Commit boundary:** `test(p2-20): add real library preview and download redeem scenarios`

---

### Task 6 — Reindex, failed-document retry, delete

**Outcome:** Real mutation paths for reindex, retry-from-failed, and delete.

**Files:**

- Add `web/e2e-real/actions.spec.ts`

**Scenarios:**

1. Reindex on indexed doc → success notice
   (`Đã đưa tài liệu vào hàng đợi lập chỉ mục.`)
2. Failed document shows failed badge; **Thử lại lập chỉ mục** enqueues retry
3. Delete with confirm dialog → row gone after refetch

**Fixture note:** create the failed document deterministically inside the dev/CI fixture
transaction using the existing `documents.state='failed'`, `document_versions`, and
`documents.current_version_id` columns. The route still receives a real authenticated
HTTP request and enqueues through production services. If those current columns cannot
produce a valid retryable row under live constraints, stop and record the exact blocker;
do not add schema/API seams.

**Commit boundary:** `test(p2-20): add real reindex retry and delete scenarios`

---

### Task 7 — Deterministic 403 / 429 / throttled upload / real 413

**Outcome:** Action error mappings and upload error/progress against real backend.

**Files:**

- Expand `web/e2e-real/upload.spec.ts` and actions/error specs
- Support helper for delay-then-continue on `**/api/v1/uploads`

**Scenarios:**

1. **403:** secondary viewer actor with no `doc.upload` attempts reindex → the real
   route returns 403 and the UI shows
   `Bạn không có quyền thực hiện thao tác này với tài liệu này.` and document remains.
   Must be real HTTP 403 from backend (observe via response listener), not hidden control.
2. **429:** with low `MARKHAND_RATE_ROUTE_PER_MINUTE`, trigger enough `reindex` calls to
   obtain real 429 + Retry-After → actionable
   `/Quá nhiều yêu cầu\. Vui lòng thử lại sau \d+ giây\./` (or minutes variant)
3. **Throttled upload progress:** `delayThenContinue` on POST `/uploads` long enough to
   observe progressbar `Đang tải lên …`, then real workers drive to indexed + preview
4. **413:** upload payload larger than process `MARKHAND_MAX_UPLOAD_BYTES` → alert with
   too-large Vietnamese copy; no phantom indexed row

**Explicit non-goals:** synthesizing 403/429/413 via `route.fulfill`; production test
endpoints; changing OpenAPI.

**Commit boundary:** `test(p2-20): add real 403 429 413 and throttled upload scenarios`

---

### Task 8 — Manifest contract tests + full-stack verification evidence

**Outcome:** Artifact/manifest validators hermetically tested; full real stack run
produces reviewable evidence; quality gates recorded.

**Files:**

- Complete `deploy/scripts/test_web_e2e_real_artifacts.py` for the T2 writer/validator,
  including checksum drift, missing scenario, nonzero skip, failed teardown, and
  secret/content canary cases
- Run the live fixture setup→cleanup→verify-clean cycle and inspect the resulting
  sanitized manifest
- Update `deploy/README.md` with the fixture/artifact CLI contract, local command, output
  location, production refusal, and sanitization warning

**Verification commands (required before Review):**

```bash
python3 deploy/scripts/test_web_e2e_real_fixture.py
python3 deploy/scripts/test_web_e2e_real_orchestration.py
# and artifact tests if split:
# python3 deploy/scripts/test_web_e2e_real_artifacts.py

DEV_STACK_MODE=full bash deploy/scripts/dev-stack-ci.sh
make check-web
make check-desktop
python3 scripts/check-architecture-boundaries.py
# API drift only if OpenAPI/client touched (should be N/A):
# pnpm --dir web api:check
# Roadmap only if catalog status changes (should be N/A while Ready→In progress/Review):
# python3 scripts/build-roadmap.py --check
```

Rust pre-push trio **only if Rust files change** (planned scope avoids this):

```bash
cargo fmt --all -- --check
cargo metadata --locked --format-version 1 --no-deps
python3 scripts/check-dependency-policy.py
```

**Commit boundary:** `test(p2-20): add artifact manifest validation and record stack evidence`
(evidence fields filled in Delivery evidence — never fabricate).

---

### Ordering and conflict rules

- Task 1 before Task 2 (orchestrator calls fixture).
- Task 3 before Tasks 4–7 (specs need support).
- Execute Tasks 4–7 sequentially because they share runtime fixtures and rate-limit state;
  each task uses its named spec file and receives task review before the next begins.
- Task 8 last; no status→Done without independent review + evidence.
- Catalog stays `Ready` until first production/test code lands, then `In progress`;
  plan moves `Planned` → `In progress` at that same moment.
- Do not regenerate roadmap while catalog status remains `Ready` (this planning commit).

## Files/modules

| Owner / boundary | Paths |
|---|---|
| Deploy orchestration (CODEOWNERS `@anhnth24`) | `deploy/scripts/web-e2e-real.sh`, `deploy/scripts/web_e2e_real_fixture.py`, `deploy/scripts/web_e2e_real_artifacts.py` (if split), `deploy/scripts/test_web_e2e_real_*.py`, `deploy/scripts/redact_secrets.py` (reuse; extend canary only if needed without weakening fail-closed) |
| CI glue (read/verify; change only if contract requires) | `deploy/scripts/dev-stack-ci.sh`; existing `.github/workflows/ci.yml` `dev-stack` job remains unchanged unless its current artifact path cannot collect the validated manifest |
| Real Playwright | `web/e2e-real/runtime.ts`, `web/e2e-real/support.ts`, `web/e2e-real/*.spec.ts`, `web/playwright.config.ts`, `web/src/test/e2eRealRuntimeConfig.test.ts`, `web/src/test/playwrightRealConfig.test.ts` |
| Mock suite (regression only) | `web/e2e/**` — do not weaken; no forceStatus in real |
| Existing runtime config (consume, do not redesign) | `MARKHAND_MAX_UPLOAD_BYTES`, `MARKHAND_RATE_ROUTE_PER_MINUTE`, `MARKHAND_PROFILE` |
| Catalog / plan | `plans/markhand-web/backlog/phase-2/issues/README.md` (Plan file link + later status), this plan |
| Out of module | `crates/**` production logic, OpenAPI, migrations, converter, pins, desktop app code, P2-21/22/23 scopes |

## Dependencies / blocks

- P2-02…14 `Done`; real 3-flow baseline on master ancestry (PR #377/#379) — unblocks start.
- Design + Ready approval via PR #394 / owner draft approval — present.
- Catalog/tracker remote sync pending PR #394 merge — does **not** block implementation
  plan authorship or delivery coding on the feature branch; remote issue URL remains
  pending.
- Compose/dev stack available for T8 full evidence.
- **Hard blocker (stop, do not invent):** any need for new production API, schema
  migration, auth bypass, or converter/pin change to make a scenario deterministic.
- P2-21/22/23 and P2-15 umbrella remain out of this PR’s outcome.

## Acceptance criteria

| # | Criterion | Implementation location | TDD RED → GREEN command | Fixture / environment | Expected evidence |
|---|---|---|---|---|---|
| A1 | Each run has unique namespace + runtime credentials; fixed seed account is not authority | `web_e2e_real_fixture.py`; `web/e2e-real/support.ts`; env export from `web-e2e-real.sh` | RED: `test_web_e2e_real_fixture.py` missing-env/refuse; GREEN: same + real login spec | Isolated Compose Postgres; fixture out-file | Fixture manifest IDs + checksum; login uses runtime email only |
| A2 | Fixture tooling refuses production profile | `web_e2e_real_fixture.py` | RED/GREEN: `python3 deploy/scripts/test_web_e2e_real_fixture.py` | `MARKHAND_PROFILE=prod` in test env | Non-zero exit; no DB writes |
| A3 | Setup/teardown idempotent + bounded; leak fails job | fixture cleanup + `verify-clean`; orchestrator trap | RED: planted leak fails verify; GREEN: cleanup clears | Dev stack | Teardown result `ok` in manifest; cleanup failure → job ≠0 |
| A4 | Supervisor fails on required process death, Playwright fail, fixture setup/cleanup fail, redactor fail, artifact validation fail, secret/content canary | `web-e2e-real.sh` + orchestration tests | RED/GREEN: `python3 deploy/scripts/test_web_e2e_real_orchestration.py` | Hermetic shims | New tests green; no raw secrets in dumps |
| A5 | Login + logout against real backend | `web/e2e-real/auth.spec.ts` | Real Playwright via orchestrator | Runtime user | Scenario pass; skipped=0 |
| A6 | Anonymous deep-link preserved through successful login | `web/e2e-real/auth.spec.ts`; relies on `RouteGuard` `?next=` | Real Playwright | Run collection path | Land on intended route post-login |
| A7 | One real backend 401 with real refresh/retry | `web/e2e-real/auth.spec.ts`; one-shot invalid bearer + `route.continue()` | Real Playwright | Runtime user with valid refresh token | Observed real 401 → real refresh 200 → retried `/auth/me` 200; no `/login` bounce |
| A8 | Collection navigation | `web/e2e-real/library.spec.ts` | Real Playwright | Run collection | Upload control visible |
| A9 | Real indexed preview | `library`/`upload` real specs | Real Playwright | Convert/index/embedding workers | Preview contains uploaded text; badge indexed |
| A10 | Real download capability + redeem | `download`/`library` real specs | Real Playwright | MinIO + capability keys | Success UI; token not in artifacts |
| A11 | Reindex + failed-document retry | `actions` real specs | Real Playwright | Failed doc fixture or real fail | Notices + state transitions |
| A12 | Delete | `actions` real specs | Real Playwright | Indexed/failed doc | Row removed |
| A13 | Deterministic real 403 action mapping | actions/error real specs | Real Playwright | Secondary viewer without `doc.upload` attempts reindex | UI permission copy; real status 403 observed; document unchanged |
| A14 | Deterministic real 429 action mapping | actions real specs | Real Playwright | Low `MARKHAND_RATE_ROUTE_PER_MINUTE` | Actionable retry-after copy |
| A15 | Throttled real upload progress → indexed preview | `upload.spec.ts` real | Real Playwright | `delayThenContinue` + workers | Progressbar then indexed preview |
| A16 | Real backend 413 | `upload.spec.ts` real | Real Playwright | Small `MARKHAND_MAX_UPLOAD_BYTES` | Accessible too-large alert; no crash |
| A17 | Network shaping never synthesizes success/authorization | support helper + review | Code review + tests forbid `fulfill` allow | N/A | Helper only `continue`/delay/abort-without-fake-200 |
| A18 | Real tests serial; required scenarios cannot skip | `playwright.config.ts` real project; specs | Config assert + CI run | Real project | `workers: 1`; skipped count 0 |
| A19 | Sanitized manifest complete; traces/screenshots restricted | artifacts module + config | Artifact unit tests + CI | Artifact dir | Manifest fields present; checksums match; no content/cred leak |
| A20 | Mock suite no regression | `web/e2e/**` untouched in intent | `make check-web` / Playwright mock job as applicable | Mock vite | Mock green |
| A21 | No dep/pin/converter/native/public API/schema change | Diff review | `git diff` review | N/A | Diff limited to deploy scripts + web e2e-real + plan/catalog; else blocker |

## Required tests / evidence

**Focused (every implementation PR):**

```bash
python3 deploy/scripts/test_web_e2e_real_fixture.py
python3 deploy/scripts/test_web_e2e_real_orchestration.py
# plus artifact tests if split
pnpm --dir web test --run   # mock/unit regression
```

**Full real stack (before Review):**

```bash
DEV_STACK_MODE=full bash deploy/scripts/dev-stack-ci.sh
```

**Repo gates:**

```bash
make check-web
make check-desktop
python3 scripts/check-architecture-boundaries.py
```

**Conditional:**

- `pnpm --dir web api:check` — only if OpenAPI/generated client touched (should not be)
- `python3 scripts/build-roadmap.py --check` + `sync-github-issues.py --dry-run` — when
  catalog status/counts change
- Rust fmt/metadata/dependency-policy — only if Rust files change

**Artifacts to retain (sanitized):** manifest JSON, Playwright JUnit/list reporter output
without traces of secrets, redacted service log excerpts on failure, fixture checksum,
teardown result. Do not upload document bodies, prompts, PII, tokens, keys, signed URLs,
cookies, or passwords.

**Baseline note:** pre-change `8 passed` orchestration + `541` web unit tests are
context only and must not be copied into Delivery evidence as proof of P2-20 completion.

## Security and migration notes

**Triggers (mandatory independent review):** auth/session, upload/converter/storage path
exercised by real E2E, runtime secrets/egress in CI fixture credentials, CI permissions /
artifact handling. Tenant ACL is exercised only for single-org foundational denial (403);
multi-org remains P2-22.

**Controls:**

- Credentials runtime-only; masked in CI; never committed
- Fixture refuses `MARKHAND_PROFILE=prod`
- Redactor fail-closed; secret/content canaries fail the job
- Download capability tokens must not appear in artifacts/logs
- Preview content may appear in live browser assertions but must not be retained in
  uploaded artifacts (canary)

**Migration:** N/A — no schema/API migration in scope. If implementers discover a hard
requirement for migration, record Blocked with exact gap; do not expand scope.

**Rollback:** revert deploy script + `web/e2e-real` + config changes; leave production
authZ and public contracts untouched.

## Out of scope

- Q&A / version modes / graph / chat history (P2-21)
- Multi-org / admin / quota matrix beyond single-action 403/429 mappings (P2-22)
- Blocking aggregator / ZAP promotion / release retention policy (P2-23)
- Production test endpoints, auth bypass, client authority, fetch mocks in real project
- Parallel real Playwright workers before isolation is measured/reviewed
- Dependency pins, converter/native boundaries, public OpenAPI/schema changes
- Changing mock suite semantics except necessary shared Playwright config hygiene
- Claiming P2-15 or Phase 2 exit complete

## Delivery evidence

### Implementation PRs

- _Pending — fill with real PR URL(s) after implementation lands._

### Recorded commit/SHA references

- Plan authorship/review commits: `0988c1c0b7fa32438ad381c20f06adaf1f13e34f`,
  `759d0cd` (implementation had not started at either SHA).
- Implementation commits / exact-SHA `dev-stack` job: _pending — do not fabricate._
- Sanitized manifest path + fixture checksum: _pending._
- Independent review outcome: _pending._

## Definition of done

- [ ] Every acceptance row (A1–A21) has reviewable evidence linked above.
- [ ] Focused Python orchestration/fixture/artifact tests green.
- [ ] Full real dev stack run (`DEV_STACK_MODE=full`) executes all required P2-20
      scenarios with skipped count 0 and teardown ok.
- [ ] `make check-web` and `make check-desktop` green for the delivery SHA.
- [ ] Architecture (and API/roadmap if applicable) checks green.
- [ ] No Rust production change; if any Rust file slipped in, Rust pre-push gates green
      and justified — otherwise revert.
- [ ] No secret/content leak in retained artifacts; redactor/canary paths proven.
- [ ] No dependency/pin/converter/native/public API/schema change; or issue Blocked with
      exact required change identified.
- [ ] Independent review complete; no unresolved Critical/Important finding.
- [ ] Catalog + plan status advanced with evidence (Ready→In progress→Review→Done) per
      `delivery.md`; merge alone is not Done.
- [ ] Remote catalog/tracker sync for P2-20 completed or explicitly still pending via
      PR #394 with no invented issue URL.
