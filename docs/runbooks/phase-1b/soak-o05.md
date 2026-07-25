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
(cannot pass). Official pass requires the canonical profile duration `1800`
with no override, canonical profile/gates SHA-256 binding, and the approved
threshold values. A copied profile, edited duration, edited bounds, or weakened
`gates.yaml` can run only as non-qualifying evidence.

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

Every prerequisite report must bind to the current full Git SHA, a clean tree,
the same Compose project, all expected image IDs, current migration manifest
hash, current compose file hash, and current index signature. O05 records
canonical F02/O02/O03/O04 report paths, SHA-256 hashes, issue IDs, and statuses.
Missing/null/stale/mismatched fields ⇒ non-pass. Stale target SHA is not allowed
by policy comment or ancestor relationship.

## Version and restore qualification paths

### Compare version pair

When `MARKHAND_SOAK_COMPARE_DATASET` is unset, preflight creates a real lineage
through public HTTP: upload version A, wait until indexed, upload version B with
multipart `documentId`, wait again, then read published version metadata and
verify compare plus both as-of windows with citations. Any HTTP, lineage,
history, timing, or retrieval mismatch fails with
`compare_dataset_unavailable`.

Operators may instead provide `MARKHAND_SOAK_COMPARE_DATASET` as JSON (or a JSON
file path) containing real
`{documentId,versionA,versionB,query,markerA,markerB,effectiveFromA,effectiveFromB,asOfA,asOfB}`.
Explicit malformed input fails closed and is never replaced automatically.

### Post-restore green endpoint

With `--invoke-o03-restore`, O05 gives O03 a mode-0600 temporary probe request.
After independent green attestation and before cleanup, O03 runs the bounded O05
probe against its live isolated green API. The probe verifies retained
citation-bearing retrieval, deleted suppression, and unauthenticated denial,
records distinct deployment/storage identities, then O03 performs normal
cleanup. O03 exit success alone is insufficient; the signed-off probe result
must also pass.

`MARKHAND_SOAK_RESTORED_API_BASE` remains available for an independently managed
green deployment. Blue and restored identities/storage signatures must be
distinct; a URL alias cannot satisfy the gate.

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
| Ingest | ≥ 300 docs/hour | `G0-CAP-INGEST-THROUGHPUT-POC` (binding) |
| RSS growth | ≤ 256 MB | profile `bounds` |
| Temp growth | ≤ 512 MB | profile `bounds` |
| Queue depth | ≤ 100 | profile `bounds` |
| DB connections | ≤ 40 | profile `bounds` |

The ingest gate deliberately binds the SLA **normal** tier on the `poc-compose`
environment, and the profile applies 0.1 ingest/second (360 docs/hour) so a pass
demonstrates the target with headroom. The **peak** tier of 1200 docs/hour lives
in `G0-CAP-INGEST-THROUGHPUT` against `on-prem-reference` (32 cores, 256 GB,
NVMe, accelerator) and is not measurable here: `compose.poc.yml` caps every
worker at 1 CPU and embeddings come from the mock service. Passing O05 qualifies
the single-org POC; it makes no Profile B capacity claim.

## Preflight seed (before timed schedule)

Official preflight uploads one fixture per format and waits until documents are
indexed/visible with expected marker hits and citations so
ingest/query/delete/reconcile actors are executable from t=0. Timed ingest
success requires `{documentId, versionId}` plus terminal convert/index/visible
completion within the bounded timeout; throughput counts completed indexed
documents, not HTTP 2xx acceptance. Delete-before-doc and compare-not-ready are
not silently tolerated as success.

## Query success

Query success requires HTTP 2xx **and** the expected result/citation behavior:
current/as-of queries must return the expected retained document marker with
citation, compare queries must return the provided compare document with
citation, and empty/wrong hits fail. Latency samples are recorded only for these
validated successes.

## Failure injection (opt-in, during active workload)

Requires `--enable-failure-injection`. Operations run on a **dedicated executor**
so dependency blip sleep/recovery never pauses event dispatch. Every scheduled
kill/blip must execute and recover (`expected==observed`, all recovered); partial
counts fail closed. Targets **only** expected POC Compose project/service names
(`worker-convert`/`worker-index` kill; `postgres`/`qdrant`/`minio` blip).
Nonzero Docker kill/stop/start/health commands fail the injection event. The
injection window is registered before the disruptive command so request failures
are classified against the active window, including failed injection attempts.

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
(unknown), never fabricated zeros. Official pass requires enough successful
samples across the full 1800s window, baseline/peak/end growth semantics,
coverage ratio ≥ 90%, no sampler errors, and command timeouts so samplers cannot
hang indefinitely.

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
# Optional: explicit pre-existing pair; otherwise public revision preflight creates it.
# export MARKHAND_SOAK_COMPARE_DATASET=/secure/path/compare-dataset.json
# Use external clean-SHA prerequisite artifacts.
export MARKHAND_O05_TRUSTED_PREREQUISITES=1
export MARKHAND_O05_F02_REPORT=/evidence/poc-f02-boot.json
export MARKHAND_O05_O02_REPORT=/evidence/o02-alerts.json
export MARKHAND_O05_O03_REPORT=/evidence/o03-restore.json
export MARKHAND_O05_O04_REPORT=/evidence/o04-release.json
export MARKHAND_O05_OUT_DIR=/evidence
bash deploy/scripts/o05-soak.sh --enable-failure-injection --invoke-o03-restore
```

## Redaction

Raw logs are pattern-redacted. Residual password/token/JWT/URL-userinfo patterns
plus `*_SECRET_KEY` / `*_ACCESS_KEY` patterns mark
`redactionScan.passed=false` and block `pass`. Document content and credentials
are not stored in report JSON or raw logs; synthetic content markers are retained
only as hashes/redacted placeholders.

## Report validation

`--validate-report` re-evaluates canonical `o05-soak.json`, corrected recovery
schema fields, redaction, and `raw-manifest.json`. A stored `status=pass` without
the canonical issue/report pointer, raw manifest, or matching raw-manifest hash
is rejected.

## Catalog honesty

Issue status stays **In progress** until an official live run produces
`o05-soak.json` with `status=pass`. Harness completion alone is not Done.
O03 promote/cutover remains deliberately disabled; O05 qualifies the distinct
attested green target before cleanup without claiming traffic cutover.
