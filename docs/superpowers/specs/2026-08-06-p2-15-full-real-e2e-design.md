# P2-15 Full Real-E2E and Blocking DAST Design

Created: 2026-08-06  
Owner decision: full parity, decomposed into four independently reviewable issues/PRs  
Umbrella: P2-15 — Contract/integration/E2E suite

## Objective

Replace P2-15's partial real-deployment smoke coverage with a fail-closed qualification
matrix that exercises the same observable product outcomes as the mock Playwright suite
against a real server, PostgreSQL, Qdrant, object storage, converter/index/embedding
workers, and built SPA. Promote OWASP ZAP from an opt-in warning report to a blocking,
reviewable gate without weakening Phase 1C denial requirements.

P2-15 remains an umbrella and becomes `Done` only after P2-20 through P2-23 are `Done`.
Each child issue has one outcome and one PR.

## Verified starting point

- Mock Playwright currently has 38 active scenarios across auth, library/actions,
  upload/indexing, Q&A/versioning, org/project scope, admin/quota, graph, and chat history.
- Real Playwright has three scenarios: login, library shell, and upload → indexed →
  preview.
- Real 3/3 evidence exists on master ancestry after PR #377 and PR #379; the older
  P2-15 catalog statement that upload remained unqualified is stale.
- ZAP ran live three times on PR #378. The latest completed report had 0 failures,
  66 passing checks, and one informational alert type (`10049`, storable/cacheable
  content) on `/`, `/robots.txt`, and `/sitemap.xml`. The catalog statement that ZAP
  never ran is stale.
- Current ZAP is opt-in, `fail_action: false`, and job-level
  `continue-on-error: true`; it is not a merge blocker.
- Existing service-log redaction is tested, but Playwright/ZAP evidence retention and
  checksums are not yet a complete closure artifact contract.

## Decomposition and dependency graph

```text
P2-20 Real E2E foundation + auth/library/upload/actions
   ├──> P2-21 Real knowledge flows
   └──> P2-22 Real multi-org security/admin

P2-18 + P2-19 + P2-20 ──> P2-21
P2-18 + 1C-12 + P2-20 ──> P2-22
P2-21 + P2-22 + 1C-13 ──> P2-23 Blocking qualification gate
P2-20 + P2-21 + P2-22 + P2-23 ──> P2-15 Done ──> P2-16 final gate
```

Readiness consequence:

- P2-20 can become `Ready` after its canonical draft is approved.
- P2-21 is blocked until P2-18 and P2-19 are `Done`.
- P2-22 is blocked until P2-18 and 1C-12 are `Done`.
- P2-23 is blocked until P2-21, P2-22, and 1C-13 are `Done`.
- P2-15 is `Blocked` as an umbrella while any child remains open.

## Shared architecture constraints

1. The tests use the real public HTTP/SSE contracts and built SPA. No production
   test-only route, authorization bypass, mock fetch registry, or client-derived
   authority may be added.
2. Deterministic setup may use dev/CI-only scripts, direct PostgreSQL fixture setup,
   existing seed commands, and local provider stubs that bind only on loopback. These
   tools must fail to start under production profile.
3. Every run receives a unique fixture namespace. Users, orgs, projects, collections,
   documents, sessions, and uploaded object keys are attributable to that run and are
   removed during bounded teardown.
4. Credentials are generated at runtime, held in masked environment/output files, and
   never written to tracked fixtures, Playwright traces, screenshots, or logs.
5. Real tests run serially until fixture isolation is independently proven. Parallelism
   may be introduced only with a separate measured change.
6. Mock tests remain the fast deterministic layer. Real parity means every mock product
   outcome has a mapped real scenario or an explicit technical exclusion; it does not
   require line-for-line duplicate test code.
7. Expected denial is asserted at both UI behavior and HTTP/SSE terminal status where
   observable. A hidden/disabled control alone is not authorization evidence.
8. Phase 1C denial suites remain authoritative for repository/service security. P2-15
   browser tests exercise consumer behavior and do not replace RLS, ACL, revoke, or load
   gates.
9. No artifact may contain document bodies, prompts, PII, tokens, keys, signed URLs,
   cookies, passwords, or unredacted service logs.

## P2-20 — Real E2E foundation + auth/library/upload/actions

### Outcome

Provide a deterministic, isolated real-browser harness and cover the foundational user
journeys that do not require multi-org or Q&A-specific fixtures.

### Components

- Extend `deploy/scripts/web-e2e-real.sh` with run-scoped fixture setup/teardown,
  child-process supervision, artifact staging, and secret-canary validation.
- Add a focused fixture tool under `deploy/scripts/` that creates runtime users and
  resource IDs from a generated run ID. It may use dev database credentials only inside
  the isolated CI stack.
- Refactor `web/e2e-real/support.ts` into small helpers for runtime credentials,
  authenticated requests, run-scoped resource lookup, and cleanup.
- Port scenario-equivalent real tests for:
  - login, logout, anonymous deep-link preservation, refresh/session recovery;
  - collection navigation, preview, download capability, delete, reindex, and retry;
  - upload progress/terminal indexing, preview contents, 413 rejection, and action
    error presentation.

### Failure behavior

- Fixture setup, teardown, a required worker exit, cleanup leak, redactor failure, or
  secret-canary match fails the job.
- Cleanup runs after test failure and records only identifiers/checksums, never content.
- A scenario that cannot be made deterministic without a production test seam fails the
  issue; it is not silently mapped back to mock coverage.

### Evidence

- Orchestration unit tests cover startup, process death, cleanup, redaction, and
  artifact validation.
- One exact-SHA CI run executes every P2-20 real scenario and retains a sanitized
  manifest with scenario names, commit, fixture checksum, duration, and outcome.

## P2-21 — Real knowledge flows

### Outcome

Exercise search, streaming Q&A, citations, version modes, graph, and persisted chat
history through real server/storage/provider boundaries.

### Components

- Seed run-scoped indexed documents with deterministic Vietnamese claims and multiple
  effective versions. Conversion/indexing must complete through real workers.
- Use the approved loopback mock embedding/provider profile already supported by the
  dev stack. Provider fallback is created by stopping or failing the provider process,
  not by a production route flag.
- Port scenario-equivalent real tests for:
  - search → result → sanitized preview;
  - streamed grounded answer → ordered SSE terminal state → citation → document preview;
  - no-answer, provider fallback, and citation revocation during an active stream;
  - current, `as_of`, compare, and history version behavior;
  - multi-project query narrowing after P2-18;
  - graph filtering/table/keyboard/deep-link behavior;
  - private per-user chat history, reload, pagination, and isolation after P2-19.

### Failure behavior

- Missing citation identity, stale/foreign version metadata, post-revoke completion,
  provider failure without the specified fallback, or a stream without a durable
  terminal state fails the scenario.
- Provider and worker processes are supervised; infrastructure death is distinguished
  from an assertion failure in the sanitized manifest.

### Evidence

- Exact-SHA real-browser run plus server integration evidence for SSE/revoke ordering.
- No skipped scenario counts as pass. Provider fallback and revoke tests prove that
  their fault actually occurred.

## P2-22 — Real multi-org security/admin

### Outcome

Prove through the browser and real API that scope changes, membership administration,
and quota/permission denials cannot expose or mutate unauthorized tenant data.

### Components

- Seed two isolated orgs, multiple projects/collections, owner/admin/member users,
  a same-org restricted collection, and bounded quota states.
- Port scenario-equivalent real tests for:
  - org list/switch and denied switch;
  - delayed org-A HTTP and SSE responses discarded after switching to org B;
  - project selection never broadening collection scope;
  - invite, role change, suspend/reactivate, remove, last-owner 409, and owner-tier 403;
  - usage values from the API, collection/action 403, upload quota 429, and stale update
    handling.
- Delay tests may use a test-runner reverse proxy or Playwright network scheduling that
  forwards the actual backend response. They may not synthesize an authorized response.

### Failure behavior

- Any stale org-A render, foreign ID/title/version metadata, successful forbidden
  mutation, quota over-admission, or missing canonical error mapping fails the run.
- Denial fixtures fail closed when setup is incomplete; they never fall back to an owner
  session.

### Evidence

- Browser assertions plus a sanitized denial manifest containing route template,
  expected/actual status, actor role, org relationship, and request ID hash.
- Independent security review is mandatory because this issue spans auth/session,
  org/RBAC/ACL, quota, and CI fixture credentials.

## P2-23 — Blocking release matrix

### Outcome

Turn the completed real suites and OWASP scan into a required, artifact-backed
qualification gate for relevant changes.

### CI design

- Add one stable required aggregator check for P2-15. It always reports a result:
  relevant changes require all child jobs; irrelevant changes validate the classifier
  decision and succeed without booting the stack.
- Relevant paths include web runtime/tests, server routes/services/OpenAPI, migrations,
  deploy stack/scripts/images, dependency locks, CI workflow/classifier, and security
  policy/filter files.
- Run the complete real Playwright matrix serially on the isolated CI stack.
- Run desktop regression because web/shared contract changes can affect the Tauri
  consumer.
- Run dependency and image scans under their existing policies.
- Run ZAP automatically for every relevant PR and master push. Remove
  `continue-on-error`, set `fail_action: true`, and prohibit skipped ZAP on a relevant
  change.

### ZAP policy

- Preserve the raw ZAP Markdown/JSON/HTML reports only after secret/content scanning.
- The verified informational alert `10049` is an explicit scoped exception, not an
  unbounded baseline expansion. Its record names owner, affected URLs, rationale,
  compensating cache controls, expiry, and retest condition.
- Every unrecognized ZAP warning/failure blocks the aggregator. Updating the rules file
  requires security review and a live before/after report.
- ZAP remains passive DAST and does not replace authenticated P2-22 scenarios or Phase
  1C denial gates.

### Artifact contract

The gate retains:

- a manifest with repository SHA/ref, tool versions, scenario inventory, durations,
  outcomes, skipped count, fixture-manifest checksum, teardown result, and artifact
  checksums;
- sanitized Playwright JUnit/HTML plus trace/screenshots only for allowlisted
  non-content pages;
- sanitized ZAP reports and dependency/image scan summaries;
- desktop regression summary.

Artifact upload fails if required files are absent, checksums mismatch, teardown fails,
or secret/content canaries match.

### Evidence

- Classifier self-tests prove every relevant path activates the gate and irrelevant
  paths produce the aggregator's explicit no-op success.
- A closure-SHA CI run executes all real scenarios, desktop, dependency/image scans,
  and blocking ZAP with zero skipped required scenario and successful teardown.
- Branch/ruleset configuration shows the stable aggregator as required before P2-23 or
  P2-15 can become `Done`.

## Catalog and lifecycle design

- Add P2-20 through P2-23 to the authoritative Phase 2 catalog using all canonical
  fields. Update the phase dependency graph, summary, phase issue count, aggregate issue
  count, generated roadmap, and GitHub tracker export.
- Revise P2-15 into a canonical umbrella entry. Correct stale claims about real upload
  and prior ZAP execution, link exact historical evidence, and set it to
  `Blocked — P2-20…23 chưa Done`.
- P2-20 is drafted as `Ready` only after Definition of Ready and canonical draft
  approval. P2-21 through P2-23 record their exact blockers and remain `Blocked`.
- Each child receives its own `plans/reports/` delivery plan only when it becomes
  `Ready`; no placeholder plan links are created during issue drafting.
- P2-15 and P2-16 cannot claim production completion merely because child PRs merge.

## Security and rollback

This program triggers mandatory review for auth/session, tenant/RBAC/ACL,
upload/converter/storage, quota, secrets/egress, dependencies, CI permissions, and
public contract usage. No exception is self-approved.

Every child PR is independently rollbackable:

- P2-20 rollback removes fixture/orchestration and foundational real tests.
- P2-21 rollback removes knowledge fixtures/tests without changing production Q&A.
- P2-22 rollback removes multi-org fixtures/tests without weakening backend controls.
- P2-23 rollback removes required-gate promotion and restores the previously documented
  warning-only policy; it does not delete historical security reports.

No child may alter converter boundaries, intentional dependency pins, PDF/Whisper
caches, production authorization semantics, or public APIs merely to make an E2E
scenario easier.

## Success criteria

The design is complete when:

1. P2-20 through P2-23 exist as canonical, independently deliverable issues.
2. All 38 current mock outcomes have a documented real scenario mapping or explicit
   technically justified exclusion reviewed in the child issue.
3. No production test seam or authorization bypass is introduced.
4. Relevant changes cannot merge unless the real matrix, blocking ZAP, desktop, scans,
   artifact validation, and teardown aggregator succeed.
5. P2-15 remains blocked until all child issues and independent security review are
   complete.
