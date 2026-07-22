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

- **Status:** Done — Docker boot + sandbox preflight evidence in `bench/markhand_web/reports/poc-f02-boot.md` (API ready, workers healthy, convert preflight ok, format smoke).
- **Plan:** Pinned API/converter/index images, compose services, health/init, non-root,
  read-only, tmpfs, dropped caps, converter no-egress, resource/secret limits.
- **Files:** `deploy/{Dockerfile.server,Dockerfile.worker,compose.poc.yml,.env.example}`,
  `deploy/scripts/poc-*.sh`, `deploy/poc/*`, `deploy/README.md`.
- **Depends:** F01 + G0-CAP/G0-SEC/G0-LIC.
- **Acceptance/tests:** Clean host boot tự động; API/worker image tách; isolation/
  UID/cap/egress/native format smoke tests.
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

- **Status:** done — merged to `master` (orchestrated branch, lifecycle fixes through `3af4c79`).
- **Plan:** Core chunking + knowledge identity/signature chứa `version_id`; PG
  chunks/FTS; separate embedding batches; Qdrant payload version/effective/current;
  extract typed claim key/value/unit/scope; incremental conflict candidate outbox;
  blocking client off async executor; deterministic upsert.
- **Files:** `workers/{index,embedding}.rs`, `services/{chunking,embedding,indexing}.rs`.
- **Depends:** I03/I05/F06 + G0-RET/G0-CAP/G1A.
- **Acceptance/tests:** Approved signature; ≤1 replay batch; no duplicate; mismatch
  before publish; golden/mock/backpressure/kill/consistency tests.
- **Security/migration:** Local approved embedding only; new signature=new generation.
  **Out:** user-selected models.

### P1B-I07 — Tombstone delete và reconcile

- **Status:** done — merged to `master` via PR #245
- **Plan:** PG tombstone first; idempotent vector/object cleanup; dry-run/repair
  missing/orphan/stale across three stores.
- **Files:** `workers/{delete,reconcile}.rs`, `services/{deletion,reconciliation}.rs`.
- **Depends:** I03/I06/F06 + recovery ADR.
- **Acceptance/tests:** Immediate read suppression; drift safely repaired; repeated
  repair, race, kill/resume matrix.
- **Security/migration:** Scoped destructive audit. **Out:** legal hold/full ACL revoke.

## Retrieval và API

### P1B-R01 — Tenant-scoped hybrid retrieval

- **Status:** Done — implementation merged via PR #252; authorization hardening and
  live PostgreSQL acceptance evidence merged via PR #254.
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

- **Status:** Done — merged via PR #256. Services `{citation,preview,download}` + bounded `BlobStore`/
  `MemoryBlobStore`; citation quotes from trusted Markdown spans; original download
  uses reconciliation parent-source metadata; exact citation ignores index generation
  activity; live PG + memory-store acceptance in `tests/citation_preview_download.rs`.
- **Plan:** Stable anchor pin logical document/version number/version ID/content hash/
  effective time/current flag; fresh auth per resolve; trusted Markdown fetch; short
  single-purpose download capability.
- **Files:** `services/{citation,preview,download}.rs`, `routes/documents.rs`,
  `storage/blob.rs`, `db/{search,download_capabilities}.rs`,
  `migrations/0018_expand_download_capabilities.sql`,
  `migrations/0019_expand_download_capability_clock.sql`.
- **Depends:** F05/F06/R01.
- **Acceptance/tests:** Quote/hash/version/anchor valid; historical permission + fresh
  ACL; delete/suspend/removal deny; IDOR, expiry/replay, multi-document/multi-version,
  PDF/PPTX/XLSX anchor tests.
- **Security/migration:** No raw bucket credential/key. **Out:** rich rendering.

### P1B-R03 — Grounded Q&A, stream và fallback

- **Status:** Done — merged via PR #257. Evidence: hermetic
  `services/qa/{mod,prompt,provider,grounding,stream}.rs`
  + `tests/qa.rs`: policy-separated untrusted framing; claims-only provider with
  bounded HTTPS/local config (no redirects/proxy; secrets/model redacted); server
  validates cite-ID subset and renders markers; current/compare/history mode rules;
  conflict notes only from authorized `RetrievalResponse.conflict_evidence`;
  extractive fallback with `[CITE-*]` neutralization; streaming is **bounded
  validated replay** (validate whole answer, then UTF-8-safe chunks via bounded
  channel + caller auth probe before each app chunk; no further chunks after deny —
  no claim of recalling bytes already handed to HTTP/kernel). No DB/ACL/lock/migration.
  Routes/SSE resume remain R05.
- **Plan:** Policy-separated prompt, untrusted passage framing, GLM, version-aware
  citation validation, current answer + history/change note, token stream,
  current unresolved-conflict warnings + resolved-history note, token stream,
  deterministic extractive fallback.
- **Files:** `services/qa/{mod,prompt,provider,grounding,stream}.rs`, `tests/qa.rs`.
- **Depends:** R01/R02 + G0-RET/G0-SEC/G1A.
- **Acceptance/tests:** Citation subset only; current claim không cite version cũ;
  compare cite old+new và đúng delta; injection không tool/scope change; provider
  outage fallback; BA/design numeric conflict warning và v2 resolution; false-positive/
  accepted-exception; fabricated/version-mix/conflict citation, timeout,
  delete-during-stream tests.
- **Security/migration:** Audit metadata only. **Out:** agents/memory/web browse.

### P1B-R04 — Collection/document/job REST API

- **Status:** Done — merged via PR #258.
- **Plan:** `/api/v1` collection POC; upload/list/get/preview/delete/reindex; immutable
  version list/get/diff/current publish; conflict list/detail/triage + evidence routes;
  job status; pagination/idempotency/error schema.
- **Files:** `routes/{collections,documents,jobs}.rs`, `api/{mod,types,error,pagination}.rs`,
  `db/{conflicts,collections,documents,jobs,document_versions}.rs` helpers, `tests/api_rest.rs`.
- **Depends:** F04/F05/I01/I03/I07/R02.
- **Acceptance/tests:** Org context + permissions; stable errors; idempotent reindex;
  HTTP contract/pagination/IDOR/malformed tests (`tests/api_rest.rs`; live PG ignored
  without `MARKHAND_TEST_DATABASE_URL`).
- **Evidence (implementation ready):**
  - Shared envelope/`ApiRejection` + `AppPath`/`AppQuery`/`AppJson`, page 1..=100 + opaque
    keyset cursors (`pageInfo`), request ID via existing auth middleware.
  - Collections list/get/create/update require `doc.upload`; update ownership/allow-list;
    viewers denied; no membership admin APIs.
  - Documents list/get, R02 preview/citation/download reuse, tombstone delete; one-txn
    reindex (lock document/current version/generation, reject tombstone,
    `enqueue_within_txn`); persisted Idempotency-Key replay for upload/reindex
    (`api_idempotency_keys`); version list/get/metadata-diff (no object keys); conflict
    list/detail/triage with lineage-checked resolution versions (stable 4xx); evidence
    authorizes each claim collection before quotes + bounded keyset pages.
  - Publish: `POST .../versions/{id}/publish` uses atomic `publish_current_version` +
    same-txn reindex enqueue with `doc.publish`.
  - Jobs get/list scoped by org + collection allow-list.
  - Upload remains `POST /api/v1/uploads` — opaque `objectId` only (no quarantine key).
- **Security/migration:** Bounded body/page, no internals; additive `0020_expand_api_idempotency`.
  **Out:** admin membership API.

### P1B-R05 — Search/ask/resumable SSE API

- **Status:** Review — implementation ready in PR #259; dependencies R01/R03/R04 are Done.
- **Plan:** Search/ask/stream routes; versioned sequence; Last-Event-ID replay;
  heartbeat/bounded buffering; auth expiry/revoke close.
- **Files:** `routes/{search,ask,events}.rs`, `api/sse.rs`, `db/sse_streams.rs`,
  `migrations/0021_expand_sse_stream_events.sql`, `tests/search_ask_sse.rs`.
- **Depends:** F05/I03/R01/R03/R04.
- **Acceptance/tests:** No lost acknowledged/duplicate sequence; bounded slow client;
  reconnect/order/expiry/revoke/worker restart.
- **Evidence (implementation ready):**
  - `POST /api/v1/search` maps bounded query/collections/version mode/limit(≤100) to R01
    `hybrid_search` with fresh `OrgContext`; stable R04 `ApiError` envelopes; response
    is authorized hits + citation locators only (no raw body/internals).
  - `POST /api/v1/ask` returns R03 grounded QA over fresh retrieval (ask limit ≤32
    pre-retrieval); provider/runtime from `AppState` (absent → extractive fallback).
  - Closed-snapshot SSE: after complete R03 answer, auth probe, then one atomic txn
    writes contiguous metadata+token+terminal events and marks closed (terminal slot
    reserved; no durable open rows). `POST /api/v1/ask/stream` and
    `GET /api/v1/events/{requestId}` only deliver durable closed snapshots with
    per-event fresh auth/collection/history probe; body cancel/backpressure ends the
    HTTP connection only (DB snapshot remains reconnectable). Not true provider
    token streaming; does not claim transport-byte recall.
  - Persisted auth scope: version mode / `requires_history`, exact collection IDs,
    cited doc/version IDs; reconnect + initial delivery revalidate before payload.
  - Expired GET → 410 `stream_expired` + bounded cascade cleanup; IDOR → 404;
    refresh-family liveness requires `expires_at > clock_timestamp()`; invalid
    Last-Event-ID (incl. bad UTF-8) → 400; missing header replays from start.
- **Security/migration:** Scoped per user/org/request, no cache; additive `0021`.
  **Out:** WebSocket / R06 OpenAPI/rate-limit/readiness / O01 telemetry.

### P1B-R06 — OpenAPI, rate limit và readiness

- **Status:** Done — merged via PR #260 into the R05 stack. Delivery of the complete
  stack to the mainline remains dependent on PR #259.
- **Plan:** Complete OpenAPI/fixtures; request IDs; CORS; IP auth/user limits; quota
  metadata; live/ready/start checks.
- **Files:** `api/openapi.rs`, OpenAPI YAML, middleware, `routes/health.rs`.
- **Depends:** R04/R05/F05 + G0-SLO.
- **Acceptance/tests:** Every route represented; readiness detects required deps/
  signature/reconciliation; 429 metadata; snapshots/rate/trusted-proxy/outage tests.
- **Evidence (implementation ready):**
  - Static `openapi/openapi.yaml` + `api/openapi.rs` inventory/drift tests for every
    wired `/api/v1` route (R02/R04/R05); security scheme, SSE `text/event-stream`,
    canonical errors; forbidden secret/object-key markers.
  - Middleware order: request ID → error envelope → CORS → rate → auth extractor.
    `X-Request-Id` UUID validate/generate/echo; exact-origin CORS (prod wildcard fail);
    in-process fixed-window limiter (IP + org/user, endpoint classes) with bounded map
    eviction; trusted-proxy XFF only when peer in CIDRs (else ignore/reject spoof).
    429 envelope + `Retry-After` + quota metadata. Config caps fail closed.
  - Health: `/live` `/ready` `/startup` + compat `/api/v1/health/*`; readiness probes
    PG/MinIO/Qdrant/config/signature/reconciliation with fake-probe hermetic tests;
    liveness unaffected; HEAD supported. No distributed limiter / O01–O02.
- **Security/migration:** Conservative CORS/proxy trust. **Out:** distributed limiter.

## Operations và release

### P1B-O01 — End-to-end telemetry và safe audit

- **Status:** Review — implementation ready in PR #264; final bounded reviewer
  verification reports zero findings. F01/F05/I03 and R06 are Done.
- **Plan:** Traces API→jobs→convert/embed/retrieval/GLM; latency/queue/conversion/
  embedding/retrieval/drift/quota/backup metrics; append-only audit.
- **Files:** `src/telemetry/**`, `services/audit.rs`, `db/audit.rs`, OTel config.
- **Depends:** F01/F05/I03 + G0-SLO.
- **Acceptance/tests:** Correlation qua async; action/deny coverage; canary secret/
  content absent; trace/cardinality/redaction/audit tests.
- **Evidence (implementation ready):**
  - Central `telemetry::{config,correlation,metrics,redact,init}`: tracing init +
    optional OTLP (config-gated; test profile never dials network; prod misconfig fails).
  - Correlation: `X-Request-Id` middleware scopes task-local context; job payload
    `request_id`/`traceparent` (v5 W3C); request/worker spans use real OTel context
    with parent/link + `.instrument(span)` (no `.enter()` across await).
  - Real OTel metrics (honour `metrics_enabled`): OTLP optional + in-memory reader;
    observable queue gauges; exact per-metric label enums (HTTP method→OTHER,
    templated routes); canary custom methods never labeled/logged raw.
  - Append-only audit: typed action/resource/outcome/reason; migration `0023`
    UPDATE/DELETE/TRUNCATE protection + revoke runtime grants; metadata scalar
    allowlist per action; mutation audit fail-closed same txn; deny durability
    with fallback (never silent ignore); auth/quota deny callsites.
  - Tests: `telemetry_audit` (async correlation, cardinality, log canary, live PG
    immutability/RLS/redaction/enqueue correlation); unit redaction/config tests.
  - Docs: `docs/conventions/{observability-audit,config-secrets}.md` +
    `deploy/dev/otel-collector.yaml` / `.env.example` OTel keys. No Grafana/O02.
- **Security/migration:** Allowlist log fields; additive `0023`. **Out:** SIEM / O02–O05.

### P1B-O02 — Dashboards, alerts và runbooks

- **Status:** Review — implementation ready in PR #265; two bounded review rounds and
  focused remediation verification report zero findings. Stacked on O01 PR #264.
- **Plan:** SLO/queue/disk/dependency alerts; runbooks jobs/parser/outage/rebuild/disk/
  GLM/key rotation.
- **Files:** `deploy/observability/**`, `docs/runbooks/**`.
- **Depends:** F02/F06/I03/O01 + G0-SLO.
- **Acceptance/tests:** Trigger từng alert; runbook detection→contain→recover→verify;
  rule validation/fault/tabletop evidence.
- **Evidence (in progress):**
  - Round-1 review fixes: explicit latency histogram boundaries; digest-pinned
    node/blackbox exporters; search-route SLO only; P99/GLM probe blocked;
    promtool check/test rules; executable runbooks against `compose.poc.yml`.
  - Thresholds: formal G0/SLA vs O02 operational policy in
    `deploy/observability/thresholds.yaml`.
  - Validator regenerates `deploy/observability/evidence/validation-report.json`.
  - Report: `bench/markhand_web/reports/p1b-o02-observability.md`.
- **Security/migration:** No tenant/document high-cardinality labels. **Out:** staffing.

### P1B-O03 — Backup/restore và migration safety

- **Status:** In Progress — control plane + hermetic/static evidence on branch
  `cursor/implement-p1b-o03-5007` (stacked on O02). Live Profile-B RPO/RTO restore
  drill still pending (Docker/services unavailable in this environment).
- **Plan:** PG PITR, MinIO version inventory, Qdrant snapshot, consistency fence/
  manifest, restore order, reconcile-before-ready, vector rebuild.
- **Files:** `deploy/backup/**`, restore/migration runbooks, restore guard.
- **Depends:** F02/F03/F06/I07 + G0-ARCH/G0-SLO.
- **Acceptance/tests:** Clean restore đạt RPO/RTO; missing/orphan detect; readiness
  false until reconcile; PG rebuild; corrupt manifest/upgrade tests.
- **Evidence (in progress):**
  - Final-round remediation: PG18 WAL-Ranges + shadow recovery verify, campaign
    identity/atomic checkpoints/cutover receipts, MinIO encrypted opaque bodies,
    Qdrant v1.18.2 schema parse + alias cutover, streaming EtM crypto, sealed
    readiness campaigns, fence opt-in + restart, TLS/credential non-argv,
    migration base-ref + SQL lexer, JSON NaN reject + appVersion range.
  - Contract suite (stateful fake CLIs) + `make check-backup`; report
    `bench/markhand_web/reports/p1b-o03-backup-restore.md`.
  - Non-claim: no live restore / Profile-B RPO/RTO pass in this environment.
- **Security/migration:** Encrypted narrow credentials; expand/cutover/contract.
  **Out:** multi-region DR.

### P1B-O04 — Vertical-slice/security release suite

- **Plan:** Clean stack, seed org/accounts; every format upload→citation; suspend/
  membership remove/delete; adversarial + fault injection.
- **Files:** `crates/server/tests/e2e/**`, POC manifest, deploy test script.
- **Depends:** F01–R06 + G0-SEC/G1A.
- **Acceptance/tests:** All formats pass; unauthorized gets no text; malicious
  rejected/contained; worker kill consistent; evidence redacted.
- **Security/migration:** High/critical blocks release. **Out:** full 1C matrix.

### P1B-O05 — Mixed-load soak và POC qualification

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
