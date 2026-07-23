# Phase 1B alert rules

Prometheus rules: [`../prometheus/markhand-rules.yml`](../prometheus/markhand-rules.yml)

Unit tests (fire → resolve): [`../prometheus/markhand-rules-test.yml`](../prometheus/markhand-rules-test.yml)

Histogram fixture checker: [`../prometheus/check_histogram_fixtures.py`](../prometheus/check_histogram_fixtures.py)

Live scrape config: [`../prometheus/prometheus.yml`](../prometheus/prometheus.yml) +
[`../compose.observe.yml`](../compose.observe.yml) (Prometheus + blackbox on
`markhand-poc_private`).

Dashboard: [`../dashboards/markhand-phase1b.json`](../dashboards/markhand-phase1b.json)
(datasource variable `${datasource}`).

## Coverage

| Alert | Signal | Runbook |
|---|---|---|
| `MarkhandApiLatencyBurn` | `markhand_http_request_duration_seconds` p95 | api-latency |
| `MarkhandQueueGrowth` | `markhand_job_queue_depth` | stuck-jobs |
| `MarkhandQueueAgeHigh` | `markhand_job_queue_age_seconds` | stuck-jobs |
| `MarkhandDiskLow` | `node_filesystem_*` (instance/device/mountpoint) | disk-exhaustion |
| `MarkhandDependencyDown` | `up{job=markhand-api}` or `probe_success{job=~markhand-(postgres\|qdrant\|minio\|embedding)}` | dependency-outage |
| `MarkhandProviderErrors` | `markhand_provider_duration_seconds` | glm-fallback |
| `MarkhandEmbeddingErrors` | `markhand_embedding_batch_duration_seconds` | dependency-outage |
| `MarkhandConversionFailures` | `markhand_conversion_duration_seconds` | converter-outbreak |
| `MarkhandReconcileDrift` | `markhand_reconcile_drift_total` | reconcile-drift |
| `MarkhandQuotaExceeded` | `markhand_quota_reservation_total{outcome="exceeded"}` | stuck-jobs |
| `MarkhandBackupStale` | O01-as-shipped `markhand_backup_age_seconds` when present (O03 owns capture/restore drill; O02 does not claim a live always-present series) | backup-restore |

## Label policy

Low-cardinality only: `route`, `job_type`, `store`, `leg`, `outcome`, `kind`,
`provider`, `job`, `instance`, `device`, `mountpoint`, `fstype`, `severity`.

## Validation / tabletop

```bash
bash deploy/scripts/o02-alert-tabletop.sh
```

Pinned images: `prom/prometheus:v2.54.1`, `quay.io/prometheus/blackbox-exporter:v0.25.0`,
optional Grafana `grafana/grafana:11.1.4`. Live path polls Prometheus
`/api/v1/alerts` after a real Compose postgres stop >2m (no synthetic “live mirror”).
