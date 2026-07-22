# P1B-O02 — Dashboards, alerts, and runbooks

Production-shaped Prometheus/Grafana/Alertmanager artifacts for Markhand Web,
stacked on O01 metric names/labels from
[`docs/conventions/observability-audit.md`](../../docs/conventions/observability-audit.md).

**Scope:** rules, dashboards, OTel→Prometheus export, synthetic alert fixtures,
tabletop evidence, and operator runbooks.  
**Out of scope:** P1B-O03 backup/restore, O04 e2e release suite, O05 soak, Rust
product telemetry changes, staffing policy.

## Layout

| Path | Purpose |
|---|---|
| `thresholds.yaml` | Machine-readable thresholds with citations into `gates.yaml` / SLA / workload profile |
| `prometheus/` | Scrape config, recording rules, alert rules |
| `alertmanager/alertmanager.example.yml` | Example routing with placeholder receivers (no real secrets) |
| `otel/collector-prometheus.yaml` | OTLP receive + Prometheus exporter (`:8889`) + forbidden-attribute drop |
| `grafana/` | Provisioning + four dashboards (`markhand-slo|queue|deps|ops`) |
| `fixtures/alerts/` | Deterministic synthetic fire/resolve cases per alert |
| `fixtures/tabletop/` | Tabletop walkthrough evidence (not a live game day) |
| `compose.observability.yml` | Optional local observability sidecars |
| `evidence/` | Validation reports |

## Threshold provenance (do not invent)

| Alert class | Value | Source |
|---|---:|---|
| Query P95 | 500 ms (0.5 s) | `bench/markhand_web/gates.yaml` `G0-SLO-QUERY-P95` |
| Query P99 | 1000 ms (1.0 s) | `bench/markhand_web/gates.yaml` `G0-SLO-QUERY-P99` |
| Queue oldest age | 120 min (7200 s) | `docs/markhand-web-sla-targets.md` |
| Queue depth warning | 600 | Derived: `G0-CAP` peak 1200 docs/h × 0.5 h |
| Disk free ratio | ≥ 0.30 | `workload-profile.yaml` `hardware.headroomPercent.disk=30` |
| Availability | ≥ 99.5% | `docs/markhand-web-sla-targets.md` |
| Error outbreaks | > 5% | 10× availability error budget (0.5%) |

Backup-failure alerts are omitted until O03 emits backup metrics.

## Security / cardinality

- Labels are bounded enums only (`thresholds.yaml` allowlists).
- Forbidden as labels: `org_id`, `user_id`, `document_id`, `job_id`, `request_id`,
  paths, URLs, filenames, queries, emails, object keys.
- Alertmanager example uses `REPLACE_WITH_SEALED_SECRET` placeholders only.
- OTel collector drops forbidden attributes before Prometheus export.
- Fixtures/tabletop set `claims_real_outage: false`.

## Validate

```bash
python3 scripts/check-observability-o02.py
python3 scripts/check-observability-o02.py --self-test \
  --json-report deploy/observability/evidence/validation-report.json
make check-observability
```

## Optional local stack

```bash
docker compose -f deploy/observability/compose.observability.yml up -d
# Grafana :3000 (admin/admin local only), Prometheus :9090, Alertmanager :9093
```

Point Markhand `MARKHAND_OTEL_EXPORTER=otlp` at `http://127.0.0.1:4317` (see
`docs/conventions/config-secrets.md`). Dev debug-only collector remains at
`deploy/dev/otel-collector.yaml`; O02 adds Prometheus export in
`deploy/observability/otel/collector-prometheus.yaml`.

## Runbooks

See [`docs/runbooks/README.md`](../../docs/runbooks/README.md) — each O02 runbook
implements detection → contain → recover → verify (+ rollback).
