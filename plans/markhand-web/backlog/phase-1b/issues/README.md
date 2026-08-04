# Phase 1B issues — Secure single-org POC

Parent plan: [`../../../phase-1b-single-org-poc.md`](../../../phase-1b-single-org-poc.md)

<!-- roadmap-default-status: blocked -->
<!-- roadmap-groups: F,I,R,O -->

**Trạng thái tổng quan (cập nhật 2026-07-31).** **24/24 Done** — foundation F01–F06,
ingest I01–I07, retrieval R01–R06, operations O01–O05 (O-chain + soak pass live
2026-07-26). **Phase 1B gate đóng** trên commit `a981fb3` + R06 hanging-soak
evidence 2026-07-31. R02–R05 Done với rust-integration trên `b5cc92c` (GitHub
Actions run
[30603158015](https://github.com/anhnth24/project-example/actions/runs/30603158015),
job
[91070008980](https://github.com/anhnth24/project-example/actions/runs/30603158015/job/91070008980)).

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
- **Plan file:** [P1B-F01 detailed implementation plan](../../../../reports/plan-260804-1617-p1b-f01-extend-server-skeleton-voi-runtime-poc.md)
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

- **Status:** Done — live boot evidence regenerated 2026-07-26 on a 24-core
  Ubuntu 22.04 host from a clean tree at `f4f33cd`:
  `poc-f02-boot.json` `passed=true`, 81 checks / 0 fails, project
  `markhand-poc-f02-20260726t121843z-1815269-17292`, clean project boot measured,
  nonzero mem/cpu/pids on every service, convert network `Internal=true` with an
  executable egress probe, narrow MinIO credential positive/negative probes, and
  the full native format smoke matrix. Report sha256 prefix `9d7214df30e57a95`.
  The run wrote its evidence outside the tree so the gate could bind to a clean
  worktree; the accepted run was then copied in, with only the recorded raw
  directory paths rewritten to repository-relative form.
- **Plan file:** [P1B-F02 detailed implementation plan](../../../../reports/plan-260804-1617-p1b-f02-poc-deployment-va-isolation-scaffold.md)
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
- **Plan file:** [P1B-F03 detailed implementation plan](../../../../reports/plan-260804-1617-p1b-f03-multi-org-ready-schema-va-immutable-migrations.md)
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
- **Plan file:** [P1B-F04 detailed implementation plan](../../../../reports/plan-260804-1617-p1b-f04-orgcontext-repositories-va-state-machine.md)
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
- **Plan file:** [P1B-F05 detailed implementation plan](../../../../reports/plan-260804-1617-p1b-f05-password-auth-rotating-sessions-va-browser-refresh-t.md)
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
- **Plan file:** [P1B-F06 detailed implementation plan](../../../../reports/plan-260804-1617-p1b-f06-fail-closed-pg-qdrant-minio-adapters.md)
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
- **Plan file:** [P1B-I01 detailed implementation plan](../../../../reports/plan-260804-1617-p1b-i01-streaming-quarantine-upload-validation.md)
- **Plan:** Multipart stream+hash; magic/extension canonical format; OOXML limits;
  PDF/audio limits; retention disposition.
- **Files:** `routes/uploads.rs`, `services/upload/{stream,sniff,archive,limits}.rs`.
- **Depends:** F04/F06 + G0-SEC/G0-CAP.
- **Acceptance/tests:** Spoof/bomb/oversize/malformed/traversal/interruption rejected
  hoặc safely quarantined; bounded memory; adversarial/property tests.
- **Security/migration:** Filename metadata only. **Out:** resumable upload/malware service.

### P1B-I02 — Atomic quota admission

- **Status:** done
- **Plan file:** [P1B-I02 detailed implementation plan](../../../../reports/plan-260804-1617-p1b-i02-atomic-quota-admission.md)
- **Plan:** Transactional reserve/finalize/refund, expiry, concurrent-job admission,
  quota headers/errors.
- **Files:** `src/db/quota.rs`, `services/quota.rs`, quota middleware.
- **Depends:** F03/F04/I01 + G0-CAP.
- **Acceptance/tests:** Concurrent requests không over-reserve; every terminal path
  settles; expiry/retry/crash/overflow tests.
- **Security/migration:** Checked arithmetic, client không sửa counter. **Out:** billing.

### P1B-I03 — Durable jobs, outbox và event log

- **Status:** done
- **Plan file:** [P1B-I03 detailed implementation plan](../../../../reports/plan-260804-1617-p1b-i03-durable-jobs-outbox-va-event-log.md)
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
- **Plan file:** [P1B-I04 detailed implementation plan](../../../../reports/plan-260804-1617-p1b-i04-isolated-converter-worker.md)
- **Plan:** Download quarantine; materialize server-derived canonical extension;
  process/cgroup limits and kill descendants; ephemeral cleanup/heartbeat/cancel.
- **Files:** `src/workers/{convert,sandbox,limits}.rs`, worker image/config.
- **Depends:** F02/I03 + G0-SEC/G0-CAP/G0-LIC.
- **Acceptance/tests:** No network/host FS; timeout kills tree; cleanup all outcomes;
  fork/disk/RAM/malformed/cancel/all-format smoke.
- **Security/migration:** Unapproved model excluded, narrow credentials. **Out:** VM sandbox.

### P1B-I05 — Idempotent conversion promotion saga

- **Status:** done — merged to `master` via PR #244 (2026-07-20).
- **Plan file:** [P1B-I05 detailed implementation plan](../../../../reports/plan-260804-1617-p1b-i05-idempotent-conversion-promotion-saga.md)
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
- **Plan file:** [P1B-I06 detailed implementation plan](../../../../reports/plan-260804-1617-p1b-i06-chunk-embedding-index-worker.md)
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
- **Plan file:** [P1B-I07 detailed implementation plan](../../../../reports/plan-260804-1617-p1b-i07-tombstone-delete-va-reconcile.md)
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
- **Plan file:** [P1B-R01 detailed implementation plan](../../../../reports/plan-260804-1617-p1b-r01-tenant-scoped-hybrid-retrieval.md)
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

- **Status:** Done — CI rust-integration SUCCESS on `b5cc92c` (run
  [30603158015](https://github.com/anhnth24/project-example/actions/runs/30603158015)/job
  [91070008980](https://github.com/anhnth24/project-example/actions/runs/30603158015/job/91070008980)):
  `live_citation_authz_expiry_replay_idor_and_immediate_deny` passed with
  worker-produced history/IDOR/delete paths; `live_minio_cleanup_guard_soak`
  passed. Prior multi-format vertical slice retained:
  `live_upload_convert_index_citation_vertical_slice` covers all
  `phase1b-mixed.yaml` ingest formats via HTTP upload → ConvertWorker/`fileconv`
  → IndexWorker → citation resolve on worker-produced IDs/artifacts/chunks.
- **Plan file:** [P1B-R02 detailed implementation plan](../../../../reports/plan-260804-1617-p1b-r02-citation-preview-va-download-authorization.md)
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

- **Status:** Done — acceptance matrix green on CI rust-integration (`b5cc92c`,
  run [30603158015](https://github.com/anhnth24/project-example/actions/runs/30603158015)/job
  [91070008980](https://github.com/anhnth24/project-example/actions/runs/30603158015/job/91070008980)):
  full `ask_grounding_matrix` passed. Ask remains intentionally fail-closed /
  extractive when structured entailment is unavailable — **does not** claim
  structured-entailment or GLM grounded. Conflict hydrate exposes
  status/resolutionNote; current warns only `open`; history emits resolution
  notes for resolved/accepted_exception/false_positive. Prior live evidence
  retained: delete/ACL-revoke mid-stream barriers
  (`live_ask_stream_slow_trickle_concurrent_delete_releases_locks`,
  `live_ask_stream_jwt_exp_membership_and_delete_barriers`);
  `live_ask_conflict_triage_then_current_and_history_matrix`;
  `live_ask_wrong_delta_and_contradiction_soak_stays_fail_closed`.
  `STRUCTURED_ENTAILMENT_AVAILABLE = false` / `force_extractive_only()` stay
  hardcoded; opt-in `MARKHAND_QA_ALLOW_UNVERIFIED_LLM` (default OFF) may emit
  `llm_unverified` with fixed warning, never grounded.
- **Plan file:** [P1B-R03 detailed implementation plan](../../../../reports/plan-260804-1617-p1b-r03-grounded-q-a-stream-va-fallback.md)
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

- **Status:** Done — all CI-runnable `api_http_contracts` tests green on
  rust-integration (`b5cc92c`, run
  [30603158015](https://github.com/anhnth24/project-example/actions/runs/30603158015)/job
  [91070008980](https://github.com/anhnth24/project-example/actions/runs/30603158015/job/91070008980)),
  including cross-tenant IDOR/403 fixture corrections. Note:
  `test-hooks`-only audit rollback tests
  (`live_patch_collection_audit_correlation_and_rollback`,
  `live_reindex_audit_failure_rolls_back_enqueue`) are excluded from the normal
  rust-integration build (feature not enabled in CI), so evidence does not
  cover that gated subset. Sol R3 upload saga retained;
  `live_http_collection_document_job_contract_matrix` asserts reindex same
  `jobId` with `created=false` on idempotent replay. Business API mutations
  gated by central `mutation_write_gate` middleware (see O03).
- **Plan file:** [P1B-R04 detailed implementation plan](../../../../reports/plan-260804-1617-p1b-r04-collection-document-job-rest-api.md)
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

- **Status:** Done — full `sse_stream_readiness` matrix green on CI
  rust-integration (`b5cc92c`, run
  [30603158015](https://github.com/anhnth24/project-example/actions/runs/30603158015)/job
  [91070008980](https://github.com/anhnth24/project-example/actions/runs/30603158015/job/91070008980)).
  Implemented: ask/job reserve-before-select on cap-1 channel; family→principal→
  fresh OrgContext → select ≤1 event under fixed pull deadline; production
  `/auth/logout` router barriers; concurrent delete trickle + `acl_mutate`
  role/collection barriers assert no new sequenced content after commit;
  delayed-producer reconnect
  (`live_ask_stream_last_event_id_purge_and_delayed_reconnect`) and
  purge/load bound
  (`live_ask_stream_maintenance_converges_under_bounded_load`) evidence.
  Production ask remains fail-closed extractive when entailment is unavailable
  (by design — see P1B-R03; not a Done blocker).
- **Plan file:** [P1B-R05 detailed implementation plan](../../../../reports/plan-260804-1617-p1b-r05-search-ask-resumable-sse-api.md)
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

- **Status:** Done — live hanging-dependency Compose soak pass 2026-07-31 on a
  24-core Ubuntu Docker host: `r06-hanging-soak.json` `status=pass`, 0 blockers,
  raw `r06-20260731T080518Z-eee30b03`. All four network readiness probes
  (`database`, `vector_store`, `object_store`, `embedding`) sustained 60s with
  correct 503 probe codes, bounded `/ready` deadlines, `/health/live` +
  `/openapi.yaml` within budget, bounded concurrent checkers, and confirmed
  restore/recovery. Hermetic router/readiness/unit coverage unchanged (Sol R2).
  Harness fix: post-pause `wait_for_hung_ready` excludes pool-drain transition
  samples before the sustain window (see `bench/markhand_web/hanging_soak/`).
- **Plan file:** [P1B-R06 detailed implementation plan](../../../../reports/plan-260804-1617-p1b-r06-openapi-rate-limit-va-readiness.md)
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

- **Status:** Done — live evidence 2026-07-26 at `f4f33cd`: `o01-telemetry.json`
  `status=pass` with 0 blockers. The async API→worker→provider canary closed all
  16 proofs (job terminal + payload `request_id`, DB audit row per request,
  exact deny audit, same-trace ingest and ask exports with the required
  `api.request`/`worker.convert`/`worker.index`/`worker.embed`/`retrieval`/
  `provider.chat` spans, unique span ids, canonical OTLP kinds, valid parent
  graph, grounded ask, clean metrics with no canary or high-cardinality label).
  Cargo telemetry suite, OTLP capture unit tests, live app-role audit test and
  the negative proof fixtures all passed. Report sha256 prefix
  `e8efc7b6975fdb4b`.
- **Plan file:** [P1B-O01 detailed implementation plan](../../../../reports/plan-260804-1617-p1b-o01-end-to-end-telemetry-va-safe-audit.md)
- **Plan:** Traces API→jobs→convert/embed/retrieval/GLM; latency/queue/conversion/
  embedding/retrieval/drift/quota/backup metrics; append-only audit.
- **Files:** `src/telemetry/**`, `services/audit.rs`, `db/audit.rs`,
  `deploy/dev/otel-collector.yaml`.
- **Depends:** F01/F05/I03 + G0-SLO.
- **Acceptance/tests:** Correlation qua async; action/deny coverage; canary secret/
  content absent; trace/cardinality/redaction/audit tests.
- **Security/migration:** Allowlist log fields. **Out:** SIEM.

### P1B-O02 — Dashboards, alerts và runbooks

- **Status:** Done — live tabletop 2026-07-26 at `f4f33cd`: `o02-alerts.json`
  `status=pass`, 31 passes / 0 fails, no blockers. Real fault executed against
  the POC stack: `MarkhandDependencyDown` fired at 150s while Postgres was
  stopped and went absent 24s after restore, both snapshots taken from the live
  Prometheus `/api/v1/alerts` (no synthetic promtool mirror). Also covered:
  promtool rule + unit tests, dashboard/datasource parameterization, runbook
  DCRV, PG restore arm-before-stop failpoint matrix, live reconcile worker
  dry-run→repair→idempotent plus the `worker-reconcile-oneshot` compose job, and
  a clean provenance + broad secret scan. Report sha256 prefix
  `56f0475a26fd174d`.
- **Plan file:** [P1B-O02 detailed implementation plan](../../../../reports/plan-260804-1617-p1b-o02-dashboards-alerts-va-runbooks.md)
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

- **Status:** Done — live blue/green drill 2026-07-26 at `f4f33cd`, run inside
  the O05 soak so it restored a loaded stack rather than an idle one:
  `o03-restore.json` `status=pass`, 0 gaps. Measured attested consistency RPO
  328s (≤ 900s), query-ready RTO 1099s (≤ 3600s) and full-vector RTO 1099s
  (≤ 14400s) — an order of magnitude above the idle-stack drill (26s/34s)
  because ~180 documents of objects are restored one at a time. The restored
  green API answered a grounded query from the
  restored stores while blue stayed fenced, promote/cutover stayed disabled
  (exit 3), the encrypted destination policy was exercised, and cleanup was
  verified before the report. Report sha256 prefix `66b5045a80925f90`.
  Promote itself remains out of scope until the API consumes durable routing
  plus an independent reconcile target-state attestation.
  **Previous write-gate context (retained):** central `middleware/write_gate.rs` /
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
  exclusive fail-closed; no pool leak).
- **Plan file:** [P1B-O03 detailed implementation plan](../../../../reports/plan-260804-1617-p1b-o03-backup-restore-va-migration-safety.md)
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

- **Status:** Done — live release gate 2026-07-26 at `f4f33cd`:
  `o04-release.json` `status=pass` with no blockers, validated through
  `MARKHAND_RELEASE_GATE=1 cargo test -p fileconv-server --test e2e_release_suite`
  (3/3). Full workload format matrix observed (csv, docx, html, pdf, png OCR,
  pptx, txt, xlsx), black-box HTTP probes against the deployed Compose API,
  unauthorized/cross-tenant denial against real foreign-org resources,
  suspend/membership/delete deny, adversarial upload reject/contain, and
  structured external worker kill → lease expiry → reclaim → replay → DB
  verification. Provenance binds the F02 project, container ids and image ids.
  Report sha256 prefix `949e14202849cf8b`.
- **Plan file:** [P1B-O04 detailed implementation plan](../../../../reports/plan-260804-1617-p1b-o04-vertical-slice-security-release-suite.md)
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

- **Status:** Done — the official 1800s run passed every gate on 2026-07-26 at
  `f4f33cd`, on a 24-core Ubuntu host, Compose project
  `markhand-poc-f02-20260726t121843z-1815269-17292`, with F02/O01/O02/O03/O04
  passing on that same commit and project. `o05-soak.json` is `status=pass` with
  no blockers; report sha256 prefix `a1a6d0e6ee57df4d`.
- **Plan file:** [P1B-O05 detailed implementation plan](../../../../reports/plan-260804-1617-p1b-o05-mixed-load-soak-va-poc-qualification.md)
- **Measured 1800s run (host: 24-core Ubuntu 22.04, Docker capped at 10 CPU):**
  - Capacity: ingest 356 documents/hour (178 of 180 uploads reached terminal
    indexed; gate ≥ 300), query p95 302 ms (≤ 500), p99 418 ms (≤ 1000).
  - Stability: RSS growth 15.6 MB (≤ 256), temp growth 0.06 MB (≤ 512), queue
    depth 0 (≤ 100), DB connections 19 (≤ 40), `unboundedGrowth` pass.
  - Resource coverage 362 of 362 expected samples, maximum gap 7.5s (12.5s
    allowed).
  - `requestErrors` 38, **all 38 inside injection windows** under both
    attributions the report records, so zero unexplained errors either way:
    34 query 503s, 2 upload 503s, 1 delete 503, 1 reconcile enqueue refused
    while the database was deliberately blipped.
  - Recovery: 2 worker kills + 1 dependency blip, every event recovered,
    expected == observed.
  - `postRestoreRetrieval` pass: the in-run O03 restore drained, produced an
    attested green endpoint and answered the retained/deleted/authz probe.
- **Defects this gate found and fixed:** the POC stack ran no consumer for
  `delete` or document-drift `reconcile` jobs, so deleted content was never
  reclaimed and any quiesced-pipeline operation stalled forever; the soak client
  reused one access token past its 900s lifetime; the restore drill proved its
  canary with a fixed question that only matched on a near-empty collection; the
  convert sandbox needed an AppArmor profile allowing `mount` and 4 GiB of
  address space on many-core hosts.
- **Gate scoping (2026-07-25, project-owner decision):** the soak used to bind
  `G0-CAP-INGEST-THROUGHPUT` (1200 docs/hour), which the gate registry defines
  for `on-prem-reference` (32 cores, 256 GB, NVMe, accelerator) and which the SLA
  document itself records as unmeasurable on local CPU. The profile also applied
  1800 docs/hour, 1.5× the peak target it was validating. O05 now binds
  `G0-CAP-INGEST-THROUGHPUT-POC` (≥ 300 docs/hour, SLA normal tier) on the new
  `poc-compose` environment, and the profile applies 360 docs/hour. The
  production peak gate is unchanged and still blocks any Profile B claim.
- **Host requirement:** the container caps alone reserve ~8.5 CPU, so the earlier
  8-vCPU WSL2 attempt starved the workers, the API and the driver against each
  other and measured 168 docs/hour. The passing run gave Docker 10 CPU on a
  24-core host. Anything at or below 8 vCPU should be expected to fail on
  throughput and latency regardless of the code.
- **Architectural notes:** `compare_dataset_unavailable` is resolved — the public
  revision preflight now builds the version pair over HTTP. `restored_api_base`
  resolves through the drill's external probe, which requires the pipeline to
  drain before the in-run restore; that only became possible once the delete and
  reconcile queues had consumers.
- **Plan:** Concurrent ingest/query/delete/reconcile against POC API per
  `phase1b-mixed.yaml`; opt-in worker-kill/dependency blip; Docker/API/PG sampling;
  evaluate binding thresholds from profile/gates/SLA; post-restore retrieval check.
- **Files:** `bench/markhand_web/soak/*`, `workloads/phase1b-mixed.yaml`,
  `reports/phase-1b-gate/o05-soak.*`, `docs/runbooks/phase-1b/soak-o05.md`,
  `deploy/scripts/o05-soak.sh`.
- **Depends:** O02/O03/O04 + G0-CAP/G0-SLO.
- **Acceptance/tests:** Unit/self-test (fake OOXML/PDF/PNG fail preflight, compare
  without dataset non-pass, async injection, partial injection counts fail,
  restored==blue/missing non-pass, retained absent / unauthorized 2xx non-pass,
  smoke≠pass); live: query p95≤500 / p99≤1000, ingest≥300 docs/h on
  `poc-compose`, RSS≤256MB / temp≤512MB / queue≤100 / DB conn≤40, recovery +
  green post-restore; duration exactly 1800.
- **Security/migration:** Synthetic/redacted, exact git/image/migration/index
  versions; injection only on expected POC project/services.
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
