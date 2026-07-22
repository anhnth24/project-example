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

```bash
docker compose --env-file deploy/.env \
  -f deploy/compose.poc.yml \
  -f deploy/observability/compose.observability.yml \
  up -d
```

If Docker is unavailable (common in CI VMs), deployability is still validated via
digest-pinned images, real service endpoints, and YAML schema — see evidence notes.

## Threshold provenance

| Kind | Examples |
|---|---|
| Formal gate/SLA | G0-SLO-QUERY-P95 (search API), SLA availability (search), queue age, disk headroom |
| Blocked | G0-SLO-QUERY-P99 filtered-query (no series); GLM blackbox probe (no endpoint) |
| O02 operational policy | error outbreak 5%, auth deny count 50/10m, drift count 10/10m, alert `for` windows |

## Layout

See `thresholds.yaml`, `prometheus/`, `blackbox/`, `grafana/`, `otel/`, `images.lock.json`.
Runbooks: `docs/runbooks/*` (detection→contain→recover→verify+rollback).
