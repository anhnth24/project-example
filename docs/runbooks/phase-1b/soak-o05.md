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

Missing/null/stale git SHA or compose project mismatch ⇒ non-pass.

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

## Failure injection (opt-in, during active workload)

Requires `--enable-failure-injection`. Worker kill runs on the profile schedule
(`killWorkerEverySeconds`); dependency blip runs mid-soak while load is active.
Targets **only** expected POC Compose project/service names
(`worker-convert`/`worker-index` kill; `postgres`/`qdrant`/`minio` blip).
Arbitrary container IDs are refused. Before/after IDs, recovery latency, and
injection-window request errors are recorded under `raw/o05-<stamp>/`.

## Post-restore retrieval

Baseline synthetic docs are created during load. Same-run O03 restore is a
**qualification checkpoint after baseline** (`--invoke-o03-restore`). Only then
does post-restore retrieval verify retained authorized docs and deleted-doc
suppression. Without a same-run restore, `postRestoreRetrieval` stays
`unknown`/`fail` — a plain deleted-id check is not post-restore evidence.

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

# Official live qualification (expected wall ~1800s + injection/recovery)
export MARKHAND_SOAK=1
export MARKHAND_SOAK_API_BASE=http://127.0.0.1:8788
export MARKHAND_SOAK_EMAIL=admin@poc.example
export MARKHAND_SOAK_PASSWORD=...          # never committed
export MARKHAND_SOAK_COLLECTION_ID=55555555-5555-5555-5555-555555555501
export MARKHAND_COMPOSE_PROJECT=markhand-poc
export MARKHAND_INDEX_SIGNATURE=...         # 64 lowercase hex
bash deploy/scripts/o05-soak.sh --enable-failure-injection
```

## Redaction

Raw logs are pattern-redacted. Residual password/token/JWT/URL-userinfo patterns
mark `redactionScan.passed=false` and block `pass`. Document content is not stored
in the report JSON.

## Catalog honesty

Issue status stays **In progress** until an official live run produces
`o05-soak.json` with `status=pass`. Harness completion alone is not Done.
