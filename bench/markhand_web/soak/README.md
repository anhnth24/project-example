# Phase-1B mixed-load soak harness

P1B-O05 delivers the soak harness, workload profiles, and pending aggregate gate
report shape. It does **not** run numeric G0 gates in this sandbox.

> Numeric G0-SLO/G0-CAP/DR/soak gates require sustained real infrastructure with
> `targetMatch=true`. This sandbox has no sustained-load/DR stack; harness output
> here is pending/null evidence only.

## Files

- `bench/markhand_web/soak/run_soak.py` - stdlib Python mixed-load driver.
- `bench/markhand_web/workloads/soak-smoke.yaml` - short harness smoke profile.
- `bench/markhand_web/workloads/soak-phase-1b.yaml` - full Phase-1B profile.
- `bench/markhand_web/reports/phase-1b-gate.py` - aggregate gate report.
- `bench/markhand_web/reports/phase-1b-gate/template.md` - markdown template.

All workload YAML is JSON-compatible YAML 1.2 and contains only synthetic,
redacted fixture text.

## Start an F02 POC stack

On a host with Docker Engine:

```bash
docker compose -f deploy/compose.poc.yml up --build
curl --fail http://127.0.0.1:${MARKHAND_POC_SERVER_PORT:-8787}/api/v1/health/ready
```

Seed a single POC org/user after the server applies migrations. The existing
seed helper inserts the org/user membership but does not commit a password:

```bash
deploy/scripts/seed-poc-org.sh
```

Use an owner bearer token from the POC/O04 seed flow, or login after a secure
local credential has been provisioned:

```bash
export MARKHAND_BASE_URL="http://127.0.0.1:${MARKHAND_POC_SERVER_PORT:-8787}"
export MARKHAND_BEARER_TOKEN="<owner access token>"

# Optional: reuse an existing collection instead of letting the harness create one.
export MARKHAND_COLLECTION_ID="<collection uuid>"
```

If the target URL or token is unset, the harness self-skips and writes a clear
pending summary instead of attempting live load.

## Run

Smoke:

```bash
python3 bench/markhand_web/soak/run_soak.py \
  --profile bench/markhand_web/workloads/soak-smoke.yaml
```

Full Phase-1B profile on real target infrastructure:

```bash
python3 bench/markhand_web/soak/run_soak.py \
  --profile bench/markhand_web/workloads/soak-phase-1b.yaml \
  --environment-id on-prem-reference \
  --target-match
```

Only use `--target-match` when the operator has verified the approved
`on-prem-reference` environment and the run duration/concurrency match the full
profile. Do not use it for local Docker smoke runs unless the report is
explicitly being kept as non-gate evidence.

Default outputs:

- `bench/markhand_web/soak/summary.json`
- `bench/markhand_web/reports/phase-1b-gate/soak.md`

## Load mix

The driver runs concurrent weighted operations until `durationSeconds`:

- `ingest`: multipart upload to `/api/v1/uploads`, then document create at
  `/api/v1/collections/{collectionId}/documents`, then job polling via
  `/api/v1/jobs/{jobId}`.
- `query_search`: filtered search via `/api/v1/search` with `collectionIds`.
- `ask`: grounded JSON answer path via `/api/v1/ask`.
- `delete`: tombstone request via `DELETE /api/v1/documents/{documentId}`;
  purge is observed through the delete worker and queue metrics.
- `reconcile`: there is no public reconcile REST route in Phase 1B. The harness
  exercises the public consistency leg by POSTing
  `/api/v1/documents/{documentId}:reindex` when a document exists, otherwise it
  probes `/api/v1/health/ready`. The actual O03 reconcile/fence leg is executed
  by the worker/runbook commands below and observed through metrics/readiness.

## Metrics monitored

The sampler polls:

- `GET /api/v1/health/ready`
- `GET /api/v1/metrics`

The harness records these `markhand_*` families when present:

- `markhand_http_requests_total`
- `markhand_http_request_duration_seconds_{bucket,sum,count}`
- `markhand_jobs_processed_total`
- `markhand_job_duration_seconds_{bucket,sum,count}`
- `markhand_jobs_in_flight`
- `markhand_jobs_queue_depth`
- `markhand_retrieval_latency_seconds_{bucket,sum,count}`
- `markhand_embedding_latency_seconds_{bucket,sum,count}`

Queue/leak proxies:

- bounded queue/in-flight behavior from `markhand_jobs_queue_depth` and
  `markhand_jobs_in_flight`;
- optional future leak proxies if exported:
  `markhand_process_resident_memory_bytes`, `markhand_temp_bytes`,
  `markhand_open_connections`, `markhand_jobs_dead_letter_total`, and
  `markhand_jobs_dead_letter_depth`;
- unavailable optional proxies are listed explicitly in the summary so the
  report does not pretend to have memory/temp/connection measurements that the
  current endpoint does not export.

## Gate mapping

The soak summary feeds the aggregate report with these mappings from the
workload profiles:

| gate id | soak metric |
|---|---|
| `G0-SLO-QUERY-P95` | `operations.query_search.durationMs.p95` |
| `G0-SLO-QUERY-P99` | `operations.query_search.durationMs.p99` |
| `G0-CAP-INGEST-THROUGHPUT` | `operations.ingest.successfulDocumentsPerHour` |
| `G0-DR-RPO` | `restore.rpoMinutes` from restore evidence |
| `G0-DR-QUERY-READY-RTO` | `restore.queryReadyRtoMinutes` from restore evidence |
| `G0-DR-FULL-VECTOR-RTO` | `restore.fullVectorRtoMinutes` from restore evidence |

The aggregate report only evaluates a gate when the source evidence is
target-valid. Synthetic/local/offline values remain `pending` with
`measuredValue=null`.

## Failure injection and restore legs

Failure steps are documented in each workload profile and recorded in the soak
summary. They are manual/operator actions, not automatic destructive operations
from the Python process.

Examples for the F02 POC stack:

```bash
# Pause converter and verify queue catch-up.
docker compose -f deploy/compose.poc.yml stop worker-convert
docker compose -f deploy/compose.poc.yml up -d worker-convert

# Put readiness behind the O03 restore/reconcile fence.
docker compose -f deploy/compose.poc.yml run --rm worker-reconcile \
  readiness-fence reconciling "soak restore leg"
docker compose -f deploy/compose.poc.yml run --rm worker-reconcile readiness-fence ready
```

For the DR leg, run the restore drill evidence generator after a real
component-loss restore:

```bash
python3 bench/markhand_web/scripts/run_restore_drill.py
```

The existing `run_restore_drill.py` in this sandbox emits an offline synthetic
smoke report with `targetMatch=false`; it is not a G0-DR pass. A real P1B gate
run must include PostgreSQL, MinIO, Qdrant, readiness fence, reconcile before
ready, and post-restore retrieval evidence from sustained infrastructure.

## Aggregate report

Render with whatever evidence is available:

```bash
python3 bench/markhand_web/reports/phase-1b-gate.py
```

Render an explicit empty-evidence pending report:

```bash
mkdir -p /tmp/markhand-empty-evidence
python3 bench/markhand_web/reports/phase-1b-gate.py \
  --evidence-root /tmp/markhand-empty-evidence \
  --summary /tmp/markhand-empty-evidence/phase-1b-summary.json \
  --report /tmp/markhand-empty-evidence/phase-1b-report.md
```

The report emits every `gates.yaml` gate as `pass`, `fail`, or `pending`.
Without target-valid evidence, all numeric P1B soak/query/ingest/restore gates
remain pending and the aggregate `targetMatch` is `false`.
