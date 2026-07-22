# P1B-O02 — Dashboards, alerts, and runbooks

Prometheus/Grafana/Alertmanager artifacts for Markhand Web on O01 metrics.
**Status: In Progress.**

## Validate (reproducible)

```bash
bash scripts/fetch-promtool.sh          # pinned 2.55.1 + SHA256 (see images.lock.json)
python3 scripts/check-observability-o02.py
python3 scripts/check-observability-o02.py --self-test
make check-observability
```

Evidence is **regenerated** at `evidence/validation-report.json` (do not hand-edit).

## Deploy overlay (requires Docker)

Merged invocation with repo-root project directory and absolute `REPO_ROOT` binds:

```bash
REPO_ROOT="$(git rev-parse --show-toplevel)"
docker compose --project-directory "$REPO_ROOT" \
  --env-file "$REPO_ROOT/deploy/.env" \
  -f "$REPO_ROOT/deploy/compose.poc.yml" \
  -f "$REPO_ROOT/deploy/observability/compose.observability.yml" \
  up -d
```

Or: `bash deploy/observability/up.sh up -d`

If Docker is unavailable (common in CI VMs), deployability is validated via
digest-pinned images, resolved bind path checks, network/OTEL matrix, and YAML
schema — see evidence notes. **No live boot is claimed when Docker is absent.**

### OTEL / networks (dev POC)

- API + index/embedding workers emit OTLP (`MARKHAND_OTEL_EXPORTER=otlp`) to
  `http://otel-collector:4317` on the `private` network.
- Convert worker stays on internal `convert` only; collector also joins `convert`
  so OTLP is reachable **without** granting convert external egress.
- Dev/POC allows internal HTTP OTLP; production contract remains HTTPS.

### Embedding alias

Overlay advertises stable DNS alias `embedding` for both `mock-embedding` (profile
`mock`) and `embedding-cpu` (profile `aiteamvn`). Blackbox probes `http://embedding:8080/health`.

## Threshold provenance

| Kind | Examples |
|---|---|
| Formal gate/SLA | G0-SLO-QUERY-P95 (search API), SLA availability (search), queue age, disk headroom (host root) |
| Blocked | filtered-query P99; GLM blackbox probe; reconcile `result=error`; named-volume disk |
| O02 operational policy | error outbreak 5%, auth deny count, drift count, dead-letter event, probe failure, alert `for` windows |

## Layout

See `thresholds.yaml`, `prometheus/`, `blackbox/`, `grafana/`, `otel/`, `images.lock.json`.
Runbooks: `docs/runbooks/*` (detection→contain→recover→verify+rollback).
