# P1B-O05 — Mixed-load soak / POC qualification

## Purpose

Measured mixed ingest/query/delete/reconcile soak against the Compose POC API,
with opt-in worker-kill + dependency blip, growth sampling, and fail-closed
numeric gates from `phase1b-mixed.yaml` + `gates.yaml` + SLA targets.

## Status semantics

| Status | Meaning |
|---|---|
| `not_run` | Default / `MARKHAND_SOAK!=1` |
| `incomplete` | Opted in but prerequisites/metrics/smoke incomplete |
| `fail` | Measured run breached a numeric/recovery/redaction gate |
| `pass` | Official live run only: duration **1800s exactly**, all prerequisites, measured gates, injection recovery, post-restore retrieval, redaction clean |

`--duration-seconds` is smoke-only and always labels `smokeNonQualifying=true`
(cannot pass). Official pass requires the profile duration `1800` with no override.

## Evidence paths (O05 only)

| Artifact | Path |
|---|---|
| Report JSON | `bench/markhand_web/reports/phase-1b-gate/o05-soak.json` |
| Report MD | `bench/markhand_web/reports/phase-1b-gate/o05-soak.md` |
| Raw samples | `bench/markhand_web/reports/phase-1b-gate/raw/o05-<stamp>/` |
| Compatibility pointer | `summary.json` (`issue=P1B-O05`, `canonicalReport=o05-soak.json`) |

Do **not** treat O04 `o04-release.json` as soak evidence. Do **not** overwrite
O04 artifacts from this harness.

## Prerequisites (fail-closed)

All required or status is non-pass:

1. **F02** `poc-f02-boot.json` with `passed=true`, `composeProject`, `imageIds`, raw/provenance
2. **O04** `o04-release.json` with `status=pass` and matching compose/images
3. **O03** `o03-restore.json` with `consistencyRpoPass=true`, `queryReadyRtoPass=true`,
   measured RPO ≤ 15m, query-ready RTO ≤ 60m, full-vector RTO ≤ 240m
4. **O02** alerts evidence passed (`failCount=0`, live fault executed / `status=pass`)

Missing/null/stale provenance or compose project mismatch ⇒ non-pass.

## Architectural blockers (honest non-pass)

These are **not** harness bugs; the harness refuses fiction:

### Compare version pair (`compare_dataset_unavailable`)

Each `POST /api/v1/uploads` creates a **new** `documentId`. Re-upload does **not**
append a second version to the same document. There is no public soak API to
create `versionB` on an existing doc. Therefore:

- Set `MARKHAND_SOAK_COMPARE_DATASET` to JSON (or a path to JSON) containing real
  `{documentId,versionA,versionB}` that the live API accepts with HTTP 2xx on
  compare search **before** the timed schedule starts.
- Never invent IDs or SQL-seed derived pairs.
- Without a verified dataset, status stays non-pass with blocker
  `compare_dataset_unavailable`.

### Post-restore green endpoint (`restored_api_base_missing` / `restored_api_same_as_blue`)

Same-run O03 restores an **isolated green** stack with promote/cutover disabled.
The blue `MARKHAND_SOAK_API_BASE` is **not** post-restore proof. O03 script exit 0
alone is not a pass.

- O03 evidence must expose `restoredApiBase` / `greenApiBase`, **or** set
  `MARKHAND_SOAK_RESTORED_API_BASE` to a reachable green host distinct from blue.
- Post-restore checks (retained authorized hit, deleted suppression, unauthorized
  denial) run **only** against that restored endpoint, using immutable document
  IDs captured before backup.
- If blue == restored or no reachable restored endpoint ⇒ gate `unknown`/`fail`.

## Fixtures

Synthetic fixtures under `bench/markhand_web/soak/fixtures/` are modeled on Rust
`tiny_*_bytes` helpers and must be **converter-accepted** (real OOXML parts,
valid PDF body, OCR-readable PNG). Preflight runs structural validation and, when
`target/debug/fileconv` (or release) is present, `fileconv one` requiring each
format’s marker in non-empty Markdown. Magic-only stubs fail closed.

## Binding thresholds

| Gate | Threshold | Source |
|---|---:|---|
| Query p95 | ≤ 500 ms | `G0-SLO-QUERY-P95` |
| Query p99 | ≤ 1000 ms | `G0-SLO-QUERY-P99` |
| Ingest | ≥ 1200 docs/hour | `G0-CAP-INGEST-THROUGHPUT` (binding) |
| RSS growth | ≤ 256 MB | profile `bounds` |
| Temp growth | ≤ 512 MB | profile `bounds` |
| Queue depth | ≤ 100 | profile `bounds` |
| DB connections | ≤ 40 | profile `bounds` |

## Preflight seed (before timed schedule)

Official preflight uploads one fixture per format and waits until documents are
indexed/visible so ingest/query/delete/reconcile actors are executable from t=0.
Delete-before-doc and compare-not-ready are not silently tolerated as success.

## Failure injection (opt-in, during active workload)

Requires `--enable-failure-injection`. Operations run on a **dedicated executor**
so dependency blip sleep/recovery never pauses event dispatch. Every scheduled
kill/blip must execute and recover (`expected==observed`, all recovered); partial
counts fail closed. Targets **only** expected POC Compose project/service names
(`worker-convert`/`worker-index` kill; `postgres`/`qdrant`/`minio` blip).

## Post-restore retrieval

Baseline IDs are captured before `--invoke-o03-restore`. Checks on the **green**
endpoint require:

1. Retained authorized hit (search or document GET 2xx)
2. Deleted ID absent from hits
3. Unauthorized token/context denied (must not 2xx)

No document content is logged.

## Sampling

Docker stats / API `/metrics` / PG connections / container temp (`du` on
allowlisted tmp paths) run on a **background sampler thread** (default 5s;
`MARKHAND_SOAK_SAMPLE_INTERVAL_SECONDS`). Missing metric series stay `null`
(unknown), never fabricated zeros.

## Run

```bash
# Hermetic unit/self-test
python3 bench/markhand_web/soak/run_soak.py --self-test
# or
bash deploy/scripts/o05-soak.sh --self-test

# Default template (honest not_run)
python3 bench/markhand_web/soak/run_soak.py \
  --profile bench/markhand_web/workloads/phase1b-mixed.yaml \
  --out bench/markhand_web/reports/phase-1b-gate

# Smoke (non-qualifying; must not pass)
export MARKHAND_SOAK=1
python3 bench/markhand_web/soak/run_soak.py \
  --profile bench/markhand_web/workloads/phase1b-mixed.yaml \
  --out bench/markhand_web/reports/phase-1b-gate \
  --duration-seconds 30

# Official live qualification (expected wall ~1800s + injection/recovery + O03)
export MARKHAND_SOAK=1
export MARKHAND_SOAK_API_BASE=http://127.0.0.1:8788
export MARKHAND_SOAK_EMAIL=admin@poc.example
export MARKHAND_SOAK_PASSWORD=...          # never committed
export MARKHAND_SOAK_COLLECTION_ID=55555555-5555-5555-5555-555555555501
export MARKHAND_COMPOSE_PROJECT=markhand-poc
export MARKHAND_INDEX_SIGNATURE=...         # 64 lowercase hex
# Required for compare gate (real API-verified pair — no invented IDs):
export MARKHAND_SOAK_COMPARE_DATASET='{"documentId":"...","versionA":"...","versionB":"..."}'
# Required for post-restore when green ≠ blue (or from O03 restoredApiBase):
export MARKHAND_SOAK_RESTORED_API_BASE=http://127.0.0.1:8789
bash deploy/scripts/o05-soak.sh --enable-failure-injection --invoke-o03-restore
```

## Redaction

Raw logs are pattern-redacted. Residual password/token/JWT/URL-userinfo patterns
mark `redactionScan.passed=false` and block `pass`. Document content is not stored
in the report JSON.

## Catalog honesty

Issue status stays **In progress** until an official live run produces
`o05-soak.json` with `status=pass`. Harness completion alone is not Done.
Compare version-pair creation and O03 promote/cutover remain architectural
blockers until a real API/path exists.
