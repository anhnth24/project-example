# Markhand observability artifacts

This directory contains static Prometheus alert rules and a Grafana dashboard for
the Phase 1B single-org POC. They build on O01's in-process Prometheus exporter and
F02's `deploy/compose.poc.yml` stack.

> **Not runtime-validated in this sandbox:** Prometheus, Alertmanager and Grafana
> are not available here. The files are structurally validated only. Alert firing,
> Alertmanager routing, dashboard rendering and datasource wiring must be verified
> on a host running the full Prometheus/Grafana stack.

## Scrape the Markhand server

O01 exposes Prometheus text at:

```text
GET /api/v1/metrics
```

The F02 POC server listens on `server:8787` inside the Compose network and on
`127.0.0.1:${MARKHAND_POC_SERVER_PORT:-8787}` from the host. A minimal Prometheus
scrape job for Compose is:

```yaml
scrape_configs:
  - job_name: markhand-server
    metrics_path: /api/v1/metrics
    static_configs:
      - targets:
          - server:8787
```

The endpoint is unauthenticated by design so Prometheus can scrape it. Keep it
network-restricted; do not publish it beyond the observability network.

`deploy/dev/otel-collector.yaml` remains the local OTLP collector scaffold. It does
not replace Prometheus scraping for these rules.

## Load alert rules

Copy or mount these files into Prometheus' `rule_files` path:

```yaml
rule_files:
  - /etc/prometheus/rules/markhand/*.rules.yml
```

Files:

- `alerts/slo.rules.yml` - query latency and query-path error-budget burn.
- `alerts/jobs.rules.yml` - queue backlog, throughput, in-flight saturation and
  failed/dead-letter job growth.
- `alerts/dependencies.rules.yml` - readiness and metrics endpoint health using
  O01 HTTP route metrics.
- `alerts/storage-pending.rules.yml` - disk and backup-age rules for metrics that
  O01 does not emit yet. Keep these disabled or routed as TODO evidence until the
  pending emitters exist.

Readiness alerts use `markhand_http_requests_total` and
`markhand_http_request_duration_seconds_count` for `/api/v1/health/ready`.
Ensure the deployment keeps polling readiness, for example via the F02 Compose
healthcheck or a platform readiness probe, so the O01 HTTP metrics have samples.

## Import the Grafana dashboard

Import `dashboards/markhand-golden-signals.json` and select the Prometheus
datasource when Grafana prompts for `DS_PROMETHEUS`.

The dashboard intentionally uses aggregate labels only:

- HTTP: `route`, `status`.
- Jobs: `job_type`, `outcome`.
- Retrieval/embedding: `stage`, `outcome`.

It does not use tenant, user, document, job-id, filename or URL labels.

## O01 metric contract

The emitted rules and dashboard use these O01 metric families and labels:

| Metric family | Labels used |
| --- | --- |
| `markhand_http_requests_total` | `route`, `method`, `status` |
| `markhand_http_request_duration_seconds_bucket` | `route`, `method`, `status`, `le` |
| `markhand_http_request_duration_seconds_count` | `route`, `method`, `status` |
| `markhand_jobs_processed_total` | `job_type`, `outcome` |
| `markhand_job_duration_seconds_bucket` | `job_type`, `outcome`, `le` |
| `markhand_jobs_in_flight` | none |
| `markhand_jobs_queue_depth` | none |
| `markhand_retrieval_latency_seconds_bucket` | `stage`, `outcome`, `le` |
| `markhand_embedding_latency_seconds_bucket` | `stage`, `outcome`, `le` |

Pending storage rules intentionally define future aggregate series:

- `markhand_disk_free_bytes{component}`
- `markhand_disk_capacity_bytes{component}`
- `markhand_backup_last_success_timestamp_seconds{component}`

`component` must stay low-cardinality, for example `postgres`, `qdrant` and
`minio`.

## SLA threshold sources

- Query P95 <= 500 ms: `docs/markhand-web-sla-targets.md`,
  `bench/markhand_web/gates.yaml` gate `G0-SLO-QUERY-P95`.
- Filtered query P99 <= 1000 ms: `docs/markhand-web-sla-targets.md`,
  `bench/markhand_web/gates.yaml` gate `G0-SLO-QUERY-P99`.
- Query availability >= 99.5% monthly: `docs/markhand-web-sla-targets.md`
  operational SLA; alert ratios use the 0.5% error budget and a 2% fast-burn page.
- Peak ingest throughput >= 1200 documents/hour:
  `bench/markhand_web/gates.yaml` gate `G0-CAP-INGEST-THROUGHPUT`.
- Oldest ingest queue age <= 120 minutes under recovery load:
  `docs/markhand-web-sla-targets.md`; recovery load is 2x normal, or
  600 documents/hour, in `bench/markhand_web/workload-profile.yaml`.
- DR RPO <= 15 minutes and disk headroom 30%:
  `docs/markhand-web-sla-targets.md`, `bench/markhand_web/gates.yaml` gate
  `G0-DR-RPO`, and `bench/markhand_web/workload-profile.yaml`.
