# Phase 1B issues — Secure single-org POC

Parent plan: [`../../../phase-1b-single-org-poc.md`](../../../phase-1b-single-org-poc.md)

<!-- roadmap-default-status: blocked -->
<!-- roadmap-groups: F,I,R,O -->

Tất cả issue bắt đầu ở **Blocked**. Chỉ chuyển `Ready` khi external gate và predecessor
ghi trong issue đã `Done`.

## External gates

| Gate | Evidence bắt buộc |
|---|---|
| G0-ARCH | ADR document/artifact, tenancy/RLS, partition, Qdrant, auth, migration, recovery |
| G0-RET | Model/dimension/normalize/chunk/signature/hybrid thresholds |
| G0-SEC | Upload allowlist/limits/quarantine/sandbox/GLM policy |
| G0-CAP | Worker/queue/concurrency/timeout/headroom |
| G0-SLO | Latency/throughput/RPO/RTO/soak numeric gates |
| G0-LIC | Model/native license inventory |
| G1A | `fileconv-knowledge` parity/extraction gate |

## Foundation

### P1B-F01 — Extend server skeleton với runtime POC

- **Status:** done
- **Plan:** Mở rộng `crates/server` API/worker skeleton từ F-02/F-07 với runtime
  dependencies, application state, graceful shutdown và các config fields đã được
  Phase 0 phê duyệt. Không tạo lại workspace/config conventions.
- **Files:** `crates/server/{Cargo.toml,src/{lib,main,config,error,state}.rs}`,
  `src/bin/worker.rs`.
- **Depends:** G0-ARCH.
- **Acceptance/tests:** API/worker compile độc lập; invalid URL/secret/limit/issuer/
  signature không start; config/env/shutdown/table tests; secrets không `Debug`.
- **Security/migration:** Unsafe defaults chỉ dev mode. **Out:** business routes/HA.

### P1B-F02 — POC deployment và isolation scaffold

- **Status:** In progress — boot-evidence **harness hardened**
  (`deploy/scripts/poc-boot-evidence.sh` + `poc_f02_boot_evidence.py --self-test`):
  O04-consumable JSON (`composeProject` / `containerIds` / `imageIds` / digests),
  allowlisted inspect (no `Config.Env`), fail-closed secret scan, executable convert
  egress probe (tool-missing ≠ pass), nonzero mem/cpu/pids required (nested nolimit/vfs
  cannot Done). Committed `poc-f02-boot.*` still awaiting **live** regeneration on a
  standard Docker host after `poc-up.sh`; do not mark Done until that evidence passes.
- **Plan:** Pinned API/converter/index images, compose services, health/init, non-root,
  read-only, tmpfs, dropped caps, converter no-egress, resource/secret limits.
- **Files:** `deploy/{Dockerfile.server,Dockerfile.worker,compose.poc.yml,.env.example}`,
  `deploy/scripts/poc-*.sh`, `deploy/scripts/poc_f02_boot_evidence.py`, `deploy/poc/*`,
  `deploy/README.md`.
- **Depends:** F01 + G0-CAP/G0-SEC/G0-LIC.
- **Acceptance/tests:** Clean host boot tự động; API/worker image tách; isolation/
  UID/cap/egress/native format smoke tests; `poc-boot-evidence.sh --self-test`.
- **Security/migration:** Narrow MinIO credentials, no bundled unlicensed model.
  **Out:** Kubernetes/HA.

### P1B-F03 — Multi-org-ready schema và immutable migrations

- **Status:** done
- **Plan:** Migrations org/auth/RBAC/groups/collections, immutable versions/artifacts,
  atomic current-published pointer, parent/version/effective lineage, chunks/FTS,
  normalized claims, conflict/evidence lifecycle, jobs/outbox, quota/audit/index;
  seed POC riêng.
- **Files:** `crates/server/migrations/000*.sql`, `src/db/models.rs`.
- **Depends:** F01 + G0-ARCH.
- **Acceptance/tests:** Mọi business row có org; immutable versions; exactly one
  current effective published version/logical document; concurrent publish/as-of/
  lineage checks; fresh + supported-upgrade migration/schema introspection.
- **Security/migration:** Files immutable sau merge; RLS theo ADR. **Out:** custom role UI.

### P1B-F04 — OrgContext, repositories và state machine

- **Status:** done
- **Plan:** Tenant-scoped repos, transaction helpers, legal document transitions;
  transaction-local RLS context nếu chọn.
- **Files:** `src/auth/context.rs`, `src/db/{orgs,collections,documents,chunks}.rs`,
  `src/services/document_state.rs`.
- **Depends:** F03 + G0-ARCH.
- **Acceptance/tests:** Không public business method thiếu context; cross-org deny;
  invalid/concurrent transition atomic; pool leakage test.
- **Security/migration:** Empty scope fail closed. **Out:** Full ACL semantics 1C.

### P1B-F05 — Password auth, rotating sessions và browser refresh transport

- **Status:** done
- **Plan:** Argon2; pinned JWT issuer/audience/alg/KID; short access; hashed rotating
  refresh family; provider interface; POC guards/audit; chốt transport theo auth ADR.
  Nếu dùng browser cookie: issue/rotate/clear `HttpOnly Secure SameSite`, CSRF token
  binding + Origin validation và OpenAPI cookie contract.
- **Files:** `src/auth/{password,jwt,session,provider,permissions,middleware}.rs`,
  `routes/auth.rs`.
- **Depends:** F03/F04 + auth ADR.
- **Acceptance/tests:** Login/refresh/logout/me; reuse revokes family; disabled user
  blocked; alg/issuer/audience/expiry/race/permission/audit tests; cookie attributes,
  CSRF missing/mismatch, cross-origin refresh/logout và cookie clearing tests nếu ADR
  chọn cookie.
- **Security/migration:** No token/password logs. **Out:** OIDC/MFA/recovery.

### P1B-F06 — Fail-closed PG/Qdrant/MinIO adapters

- **Status:** done
- **Plan:** Pools, opaque key builder, quarantine/trusted namespace, deterministic
  points, versioned collection, mandatory org/collection filters, typed errors.
- **Files:** `src/storage/{keys,minio,qdrant}.rs`, `src/db/pool.rs`,
  `services/index_signature.rs`.
- **Depends:** F02/F04 + G0-ARCH/G0-RET/G1A.
- **Acceptance/tests:** Missing/empty filter rejected; no filename in key; payload has
  all identities; real-service contracts, traversal/fuzz, deterministic vectors.
- **Security/migration:** No public key, least privilege. **Out:** generic backend trait.

## Ingest và jobs

### P1B-I01 — Streaming quarantine upload validation

- **Status:** done
- **Plan:** Multipart stream+hash; magic/extension canonical format; OOXML limits;
  PDF/audio limits; retention disposition.
- **Files:** `routes/uploads.rs`, `services/upload/{stream,sniff,archive,limits}.rs`.
- **Depends:** F04/F06 + G0-SEC/G0-CAP.
- **Acceptance/tests:** Spoof/bomb/oversize/malformed/traversal/interruption rejected
  hoặc safely quarantined; bounded memory; adversarial/property tests.
- **Security/migration:** Filename metadata only. **Out:** resumable upload/malware service.

### P1B-I02 — Atomic quota admission

- **Status:** done
- **Plan:** Transactional reserve/finalize/refund, expiry, concurrent-job admission,
  quota headers/errors.
- **Files:** `src/db/quota.rs`, `services/quota.rs`, quota middleware.
- **Depends:** F03/F04/I01 + G0-CAP.
- **Acceptance/tests:** Concurrent requests không over-reserve; every terminal path
  settles; expiry/retry/crash/overflow tests.
- **Security/migration:** Checked arithmetic, client không sửa counter. **Out:** billing.

### P1B-I03 — Durable jobs, outbox và event log

- **Status:** done
- **Plan:** Versioned payload, transactional outbox, leased SKIP LOCKED claims,
  heartbeat/retry/checkpoint/cancel/dead-letter/idempotency/sequenced events.
- **Files:** `src/jobs/**`, `src/db/jobs.rs`.
- **Depends:** F03/F04 + G0-CAP.
- **Acceptance/tests:** Commit/enqueue không split; lease reclaimed; duplicate harmless;
  kill/checkpoint/claim/dead-letter/cancel/outbox replay.
- **Security/migration:** IDs only, no content/secrets; backward-readable payloads.
  **Out:** Kafka/Redis queue.

### P1B-I04 — Isolated converter worker

- **Status:** done
- **Plan:** Download quarantine; materialize server-derived canonical extension;
  process/cgroup limits and kill descendants; ephemeral cleanup/heartbeat/cancel.
- **Files:** `src/workers/{convert,sandbox,limits}.rs`, worker image/config.
- **Depends:** F02/I03 + G0-SEC/G0-CAP/G0-LIC.
- **Acceptance/tests:** No network/host FS; timeout kills tree; cleanup all outcomes;
  fork/disk/RAM/malformed/cancel/all-format smoke.
- **Security/migration:** Unapproved model excluded, narrow credentials. **Out:** VM sandbox.

### P1B-I05 — Idempotent conversion promotion saga

- **Status:** done — merged to `master` via PR #244 (2026-07-20).
- **Plan:** Checkpoint download/convert/stage/promote/DB/cleanup; immutable version;
  publish/current pointer riêng với draft/latest upload; index outbox;
  compensation/refund.
- **Files:** `workers/convert.rs`, `services/{conversion,promotion,artifacts}.rs`,
  `db/document_versions.rs`.
- **Depends:** I01–I04/F06/G1A.
- **Acceptance/tests:** Retry tạo một visible version/job; trusted chỉ sau success;
  fault injection mọi cross-store step; immutable checks.
- **Security/migration:** Never overwrite original; ACL inherited. **Out:** user merge.

### P1B-I06 — Chunk/embedding/index worker

- **Status:** Done — Sol R2 evidence green: multi-generation
  `lifecycle_refresh` (one idempotent job per materialized generation; no
  active-generation fallback); Index↔LifecycleRefresh claim fairness
  (ConvertWorker atomic pattern); mixed-scope filter-only Qdrant update (has_id
  + org/collection/version, no body `points`). LiveEnv dual-role
  (`markhand_app`). Local:
  `cargo test -p fileconv-server --test index_worker -- --include-ignored`
  → 10 ok (natural A→B, multi-gen demote + idempotent replay, fairness ≤2
  `run_once`, mixed-scope, race, retry).
- **Plan:** Core chunking + knowledge identity/signature chứa `version_id`; PG
  chunks/FTS; separate embedding batches; Qdrant payload version/effective/current;
  extract typed claim key/value/unit/scope; incremental conflict candidate outbox;
  blocking client off async executor; deterministic upsert.
- **Files:** `workers/{index,embedding}.rs`, `services/{chunking,embedding,indexing}.rs`.
- **Depends:** I03/I05/F06 + G0-RET/G0-CAP/G1A.
- **Acceptance/tests:** Approved signature; ≤1 replay batch; no duplicate; mismatch
  before publish; golden/mock/backpressure/kill/consistency tests;
  `live_index_worker_replay_is_idempotent`;
  `live_index_worker_stale_version_does_not_mark_current_indexed`.
- **Security/migration:** Local approved embedding only; new signature=new generation.
  **Out:** user-selected models.

### P1B-I07 — Tombstone delete và reconcile

- **Status:** Done — merged via PR #245; #282 fixed reconcile audit `request_id`
  length so `live_reconcile_repairs_orphan_vectors` /
  `live_reconcile_dead_letter_staging_gc` pass under rust-integration. ADR 0015
  (purge retention semantics) remains Proposed — wording follow-up only, not a
  blocker for the delete/reconcile acceptance matrix already covered by live tests.
- **Plan:** PG tombstone first; idempotent vector/object cleanup; dry-run/repair
  missing/orphan/stale across three stores.
- **Files:** `workers/{delete,reconcile}.rs`, `services/{deletion,reconciliation}.rs`.
- **Depends:** I03/I06/F06 + recovery ADR.
- **Acceptance/tests:** Immediate read suppression; drift safely repaired; repeated
  repair, race, kill/resume matrix.
- **Security/migration:** Scoped destructive audit. **Out:** legal hold/full ACL revoke.

## Retrieval và API

### P1B-R01 — Tenant-scoped hybrid retrieval

- **Status:** done — PR #252 + authorization hardening PR #254 merged; hermetic
  unit acceptance in `services/retrieval` and gated PG tests in `tests/retrieval.rs`.
- **Plan:** Resolve scope + current/as-of/compare/history mode; query embed; parallel
  Qdrant/FTS với version filter; knowledge merge/rerank; PG hydration/recheck
  state/ACL/version; hydrate only conflict evidence whose both sides remain authorized.
- **Files:** `services/retrieval/{mod,vector,fts,hydrate}.rs`, `db/search.rs`.
- **Depends:** F04/F06/I06 + G0-RET/G1A.
- **Acceptance/tests:** Empty scope deny; stale vector no text; current không trả
  superseded version; as-of resolve đúng effective version; compare/history cùng
  lineage; golden quality/cross-scope/deleted/one-leg outage/latency tests.
- **Security/migration:** Text only after authorized hydration. **Out:** new reranker.

### P1B-R02 — Citation, preview và download authorization

- **Status:** In progress — multi-format vertical slice green on live PG/MinIO/
  Qdrant: `live_upload_convert_index_citation_vertical_slice` covers
  all `phase1b-mixed.yaml` ingest formats
  (csv/docx/html/pdf/png/pptx/txt/xlsx) via HTTP upload →
  ConvertWorker/`fileconv` → IndexWorker → citation resolve on
  worker-produced IDs/artifacts/chunks; no SQL seed of
  versions/artifacts/chunks; shared embedding plan/signature. Concurrent
  redemption barrier + expiry/IDOR/delete/suspend/membership deny covered by
  `live_citation_authz_expiry_replay_idor_and_immediate_deny` (still SQL-seeds
  derived artifacts for history ACL paths). Remaining for Done: history
  ACL/IDOR/delete-deny matrices driven only by worker-produced artifacts;
  MinIO cleanup guard soak evidence.
- **Plan:** Stable anchor pin logical document/version number/version ID/content hash/
  effective time/current flag; fresh auth per resolve; trusted Markdown fetch; short
  single-purpose download capability.
- **Files:** `services/{access,citation,preview,download}.rs`, `routes/documents.rs`,
  `migrations/0018_expand_download_capability_redemptions.sql`,
  `tests/{citation_authz_matrix.rs,common/fixtures.rs}`.
- **Depends:** F05/F06/R01.
- **Acceptance/tests:** Quote/hash/version/anchor valid; historical permission + fresh
  ACL; delete/suspend/removal deny; IDOR, expiry/replay, multi-document/multi-version,
  PDF/PPTX/XLSX anchor tests.
- **Security/migration:** No raw bucket credential/key. **Out:** rich rendering.

### P1B-R03 — Grounded Q&A, stream và fallback

- **Status:** Review — ask now attempts injectable ChatProvider
  (Static/Failing/Timeout) but never claims GLM grounded while structured
  entailment is unavailable. Conflict hydrate exposes status/resolutionNote;
  current warns only `open`; history emits resolution notes for
  resolved/accepted_exception/false_positive. Remaining for Done: live router SSE
  consume + delete-between-batches `citation_revoked`; triage-then-current/history
  matrix on real DB; wrong-delta/same-topic contradiction soak through ask path.
- **Plan:** Policy-separated prompt, untrusted passage framing, GLM, version-aware
  citation validation, current answer + history/change note, token stream,
  current unresolved-conflict warnings + resolved-history note, token stream,
  deterministic extractive fallback.
- **Files:** `services/qa/{mod,prompt,provider,grounding,stream}.rs`,
  `services/stream_auth.rs`, `routes/ask.rs`, `tests/ask_grounding_matrix.rs`.
- **Depends:** R01/R02 + G0-RET/G0-SEC/G1A.
- **Acceptance/tests:** Citation subset only; current claim không cite version cũ;
  compare cite old+new và đúng delta; injection không tool/scope change; provider
  outage fallback; BA/design numeric conflict warning và v2 resolution; false-positive/
  accepted-exception; fabricated/version-mix/conflict citation, timeout,
  delete-during-stream tests.
- **Security/migration:** Audit metadata only. **Out:** agents/memory/web browse.

### P1B-R04 — Collection/document/job REST API

- **Status:** In progress — Sol R3 upload saga retained; live
  `live_http_collection_document_job_contract_matrix` asserts reindex same
  `jobId` with `created=false` on idempotent replay. Business API mutations are
  gated by central `mutation_write_gate` middleware (see O03), not per-handler
  copies. Remaining for Done: broader cross-tenant resource IDOR suite beyond
  collections; publish/download/citation HTTP coverage in the same contract
  matrix; full status/schema matrix vs OpenAPI; live Sol R3 barrier evidence on
  CI agent.
- **Plan:** `/api/v1` collection POC; upload/list/get/preview/delete/reindex; immutable
  version list/get/diff/current publish; conflict list/detail/triage + evidence routes;
  job status; pagination/idempotency/error schema.
- **Files:** `routes/{collections,documents,jobs}.rs`, `api/{types,error,pagination}.rs`,
  `tests/api_http_contracts.rs`.
- **Depends:** F04/F05/I01/I03/I07/R02.
- **Acceptance/tests:** Org context + permissions; stable errors; idempotent reindex;
  HTTP contract/pagination/IDOR/malformed tests.
- **Security/migration:** Bounded body/page, no internals. **Out:** admin membership API.

### P1B-R05 — Search/ask/resumable SSE API

- **Status:** In progress — Sol R3 hardening in flight. Implemented: ask/job
  reserve-before-select on cap-1 channel (await client drain with no DB locks),
  then family→principal→fresh OrgContext (permissions + collection ACL) → select
  ≤1 event under fixed pull deadline → non-blocking permit enqueue; production
  `/auth/logout` router barriers (≤1 in-flight, no buffered batch after commit);
  concurrent delete trickle + `acl_mutate` role/collection barriers assert no new
  sequenced content after commit. Gaps remaining for Done: delayed-producer
  reconnect matrix green on CI agent; live purge/load bound evidence; production
  ask still often extractive when entailment fail-closed.
- **Plan:** Search/ask/stream routes; versioned sequence; Last-Event-ID replay;
  heartbeat/bounded buffering; auth expiry/revoke close.
- **Files:** `routes/{search,ask,events}.rs`, `api/{sse,last_event_id}.rs`,
  `db/ask_streams.rs`, `services/qa/{ask_stream,provider,stream}.rs`,
  `services/stream_auth.rs`,
  `migrations/0024_expand_ask_stream_sessions.sql`,
  `migrations/0025_backfill_event_log_ids_ask_stream_ops.sql`.
- **Depends:** F05/I03/R01/R03/R04.
- **Acceptance/tests:** No lost acknowledged/duplicate sequence; bounded slow client;
  reconnect/order/expiry/revoke/worker restart; zero post-revoke content; durable
  terminal/control; Last-Event-ID validation; retention purge; provider framing;
  lifecycle lease/recovery.
- **Security/migration:** Scoped per user/org/job, no cache. **Out:** WebSocket.

### P1B-R06 — OpenAPI, rate limit và readiness

- **Status:** In progress — Sol R2 complete for rate/readiness/OpenAPI; kept open with
  R05. Implemented: outer readiness timeout reports in-progress probe code; hanging
  probe router matrix (code+deadline); baseline IP shares ceil `RateLimitRejected`;
  OpenAPI `/openapi.yaml` 429; hermetic
  `concurrent_checkers_share_ceil_and_stay_bounded` for shared-ceil + hard-cap
  cardinality under concurrent pressure; `pnpm --dir web api:check` regenerates
  TS client from OpenAPI (429 sweep already in `contract.ts`). Gaps remaining
  for Done: Compose-stack hanging soak on a Docker host.
- **Plan:** Complete OpenAPI/fixtures; request IDs; CORS; IP auth/user limits; quota
  metadata; live/ready/start checks.
- **Files:** `api/openapi.rs`, OpenAPI YAML, `middleware/**`, `routes/health.rs`,
  `routes/rate_limit_guard.rs`, `services/readiness.rs`.
- **Depends:** R04/R05/F05 + G0-SLO.
- **Acceptance/tests:** Every route represented two-way; readiness detects required
  deps/signature/reconciliation with bounded deadlines; 429 metadata; trusted-proxy/
  outage tests.
- **Security/migration:** Conservative CORS/proxy trust. **Out:** distributed limiter.

## Operations và release

### P1B-O01 — End-to-end telemetry và safe audit

- **Status:** In progress — Sol R3 final fixes (no commit/push; no more reviewer):
  privileged idempotent POC `db-bootstrap` before migrate; `0028` owns audit_log +
  both trigger fns + exact SELECT/INSERT grants; Compose pre-O01 volume upgrade;
  OTLP kinds INTERNAL=1…CONSUMER=5; real span lifecycle (worker emit in scope);
  bounded shutdown (stop claim → await run_once grace → flush per remaining
  deadline); central typed route audit matrix + same-txn enqueue/audit; evidence
  deny-by-request + named spans/parent graph + negative fixtures. Keep
  `in_progress` until rebuilt POC full async evidence passes.
- **Plan:** Traces API→jobs→convert/embed/retrieval/GLM; latency/queue/conversion/
  embedding/retrieval/drift/quota/backup metrics; append-only audit.
- **Files:** `src/telemetry/**`, `services/audit.rs`, `db/audit.rs`,
  `deploy/dev/otel-collector.yaml`.
- **Depends:** F01/F05/I03 + G0-SLO.
- **Acceptance/tests:** Correlation qua async; action/deny coverage; canary secret/
  content absent; trace/cardinality/redaction/audit tests.
- **Security/migration:** Allowlist log fields. **Out:** SIEM.

### P1B-O02 — Dashboards, alerts và runbooks

- **Status:** In progress — Sol R3: JSON fallback redaction (malformed/truncated/
  prefixed/multi-record), PG restore arm-before-stop + failpoint harness,
  reconcile oneshot requires document UUID before DB + scoped claim, exact
  `worker-reconcile-oneshot` compose evidence (or honest deployment gap). Catalog
  stays In progress while O01 is not Done and full backup/restore remains O03.
  Evidence: `bench/markhand_web/reports/phase-1b-gate/o02-alerts.*`.
- **Plan:** SLO/queue/disk/dependency alerts; runbooks jobs/parser/outage/rebuild/disk/
  GLM/key rotation.
- **Files / scope:** `deploy/observability/**`, `docs/runbooks/phase-1b/**`,
  `deploy/scripts/o02-alert-tabletop.sh`, `deploy/scripts/o02-pg-restore-guard.sh`,
  `deploy/scripts/redact_secrets.py`, `deploy/scripts/test_redact_secrets.py`,
  `deploy/compose.poc.yml` (`worker-reconcile-oneshot` profile / job),
  `crates/server/src/{bin/worker.rs,workers/reconcile.rs,jobs/**,db/jobs.rs}`,
  `crates/server/tests/deletion_reconcile.rs` (live reconcile worker drills).
- **Depends:** F02/F06/I03/O01 + G0-SLO.
- **Acceptance/tests:** Trigger từng alert; runbook detection→contain→recover→verify;
  rule validation/fault/tabletop evidence; compose oneshot dry-run/repair/clean or
  documented deployment gap.
- **Security/migration:** No tenant/document high-cardinality labels. **Out:** staffing.

### P1B-O03 — Backup/restore và migration safety

- **Status:** In progress — Sol round-3 merge-safety retained. **Write-gate
  architecture (Sol R2):** central `middleware/write_gate.rs` /
  `mutation_write_gate` on all `/api/v1/*` except health/metrics/OpenAPI;
  shared advisory lock `7303003` held through entire `next.run` (ask/stream
  session init covered; lock released after `Response` is built, not for the
  SSE body lifetime); background/producer use RAII
  `acquire_background_mutation_guard` across quota sweep, ask maintenance, and
  each append txn (no check-then-release TOCTOU); honest `ops_fence_active` vs
  `ops_fence_check_failed`. Detector (`write_gate_contract.py`) requires
  ask-stream append guard + negative fixtures (comment-only decoys fail). Live:
  `live_central_write_gate_matrix_refuses_business_side_effects` (incl.
  ask/stream no session init) and
  `live_write_gate_advisory_lock_concurrency_contract` (shared blocks exclusive;
  exclusive fail-closed; no pool leak). Evidence: hermetic + live tests; raw
  `o03-restore.*` stamp still pre-dates full drill re-run. **Exact gaps:**
  (1) promote/cutover disabled until API consumes durable routing + independent
  reconcile target-state attestation; (2) encrypted backup destination not
  exercised (POC `explicit_poc_tmp_only` policy). `consistencyRpoPass` /
  `queryReadyRtoPass` remain null. Re-run `o03-bluegreen-restore-drill.sh` on
  Docker host to refresh raw passes.
- **Plan:** PG PITR, MinIO version inventory, Qdrant snapshot, consistency fence/
  manifest, restore order, reconcile-before-ready, vector rebuild.
- **Files:** `deploy/backup/**`, `deploy/scripts/o03-bluegreen-restore-drill.sh`,
  `deploy/scripts/o03-report-from-raw.py`,
  `docs/runbooks/phase-1b/backup-restore-o03.md`.
- **Depends:** F02/F03/F06/I07 + G0-ARCH/G0-SLO.
- **Acceptance/tests:** Clean restore đạt RPO/RTO; missing/orphan detect; readiness
  false until reconcile; PG rebuild; corrupt manifest/upgrade tests.
- **Security/migration:** Encrypted narrow credentials; expand/cutover/contract.
  **Out:** multi-region DR.

### P1B-O04 — Vertical-slice/security release suite

- **Status:** In progress — harness complete (`run_o04_release_suite.py` is
  evaluate source of truth; Rust `e2e_release_suite` calls
  `--validate-report`). Default evidence honest `not_run` in
  `o04-release.json` (never O05 `summary.json`). Suites are **in-process**
  workers against PG/MinIO/Qdrant endpoints — not Compose API HTTP.
  Live `pass` still blocked. Exact blockers: (1) `MARKHAND_E2E!=1` / no POC
  Compose project containers in this environment; (2) F02
  `poc-f02-boot.json` must be live-regenerated `passed=true` **with** matching
  `composeProject` + `imageIds` (harness emits those fields; committed JSON is
  still pre-harness); (3) `MARKHAND_INDEX_SIGNATURE` 64-hex; (4) full workload
  format matrix including PNG OCR (`phase1b-mixed.yaml`).
- **Plan:** Clean stack, seed org/accounts; every format upload→citation; suspend/
  membership remove/delete; adversarial + fault injection.
- **Files:** `bench/markhand_web/scripts/run_o04_release_suite.py`,
  `deploy/scripts/o04-release-suite.sh`,
  `crates/server/tests/{e2e_release_suite,retrieval_vertical_slice}.rs`,
  `docs/runbooks/phase-1b/release-suite-o04.md`,
  `bench/markhand_web/reports/phase-1b-gate/o04-release.*`.
- **Depends:** F01–R06 + G0-SEC/G1A.
- **Acceptance/tests:** All workload formats pass; unauthorized gets no text;
  malicious rejected/contained; worker kill consistent; evidence redacted;
  self-test rejects multi-filter command shapes +
  missing/skipped/ignored/zero-test/partial/high-critical/F02 mismatch.
- **Security/migration:** High/critical blocks release. **Out:** full 1C matrix.

### P1B-O05 — Mixed-load soak và POC qualification

- **Status:** In progress — soak harness never emits `pass` unless every numeric
  gate is explicitly `pass`; default/`MARKHAND_SOAK=1` alone → `not_run`/`incomplete`.
  Numeric soak/restore qualification not claimed.
- **Plan:** Ingest/query/delete/reconcile mixed load + failures; monitor leaks/queue;
  restore; aggregate gate report.
- **Files:** `bench/markhand_web/{soak,workloads,reports/phase-1b-gate}*`.
- **Depends:** O02/O03/O04 + G0-CAP/G0-SLO.
- **Acceptance/tests:** Numeric gates pass; no unbounded memory/temp/connection/queue;
  recovery/worker/dependency/restore/post-restore retrieval evidence.
- **Security/migration:** Synthetic/redacted, exact versions recorded.
  **Out:** production/multi-org.

## Critical path và release gate

```text
Phase 0 + 1A → F03/F04/F06 → I01/I03/I04 → I05 → I06
→ R01/R02/R03 → R04/R05/R06 → O04/O03 → O05
```

Phase 1B chỉ đóng khi 24 issue, mọi external gate, per-format vertical slice,
checkpoint replay, adversarial containment, immediate delete/suspend suppression,
OrgContext/fail-closed filters, reconciliation, clean restore, soak và secret-safe
telemetry đều đạt. Release phải được ghi rõ là **trusted single-org POC**.
