# Configuration and secrets

`fileconv-server` uses typed configuration. Precedence is:

```text
safe defaults < MARKHAND_CONFIG_FILE JSON < MARKHAND_* environment
```

`MARKHAND_PROFILE` is `dev`, `test`, or `prod`. Dev/test default to loopback
`127.0.0.1:8787`; production requires an explicit non-loopback bind address and
non-development database URL. Invalid profile, address or production configuration
fails before server/worker work begins.

The running server additionally requires `MARKHAND_DATABASE_URL`,
`MARKHAND_QDRANT_URL`, and `MARKHAND_MINIO_URL`; `--check-config` validates these
runtime endpoints too. Production requires `sslmode=require` for PostgreSQL; the
server's Rustls connector validates the peer certificate and hostname using the
native root store. Qdrant and MinIO require HTTPS. Local development may use the
plain HTTP and PostgreSQL endpoints in `deploy/dev/.env.example`.

## Secrets

- Never commit `.env`, API keys, database credentials, token signing keys, signed URLs
  or customer hostnames. Use [`deploy/dev/.env.example`](../../deploy/dev/.env.example)
  as the non-secret template.
- Production uses orchestrator secret mounts or environment references. Do not put a
  production secret in JSON config checked into source.
- Secret values use redacted `Debug`; errors/logs state the invalid field without
  echoing its value.
- Rotate secret by deploying a new reference, verify health, then revoke old material.
  Rotation implementation/secret manager selection is Phase 4.

## Profiles

| Profile | Bind default | Credential rule |
|---|---|---|
| `dev` | loopback | non-production local values only |
| `test` | loopback | fixture/ephemeral values only |
| `prod` | explicit required | no loopback/default database credential |

## Telemetry / OpenTelemetry

Optional. Defaults keep exporters off so unit/integration tests never dial a network
collector.

| Variable | Default | Notes |
|---|---|---|
| `MARKHAND_OTEL_EXPORTER` | `none` | `none` or `otlp` |
| `MARKHAND_OTEL_EXPORTER_OTLP_ENDPOINT` | unset | Required when exporter=`otlp` (HTTPS in prod; gRPC TLS via `tls-ring`/`tls-roots`) |
| `MARKHAND_OTEL_SERVICE_NAME` | `markhand` | Resource service name |
| `MARKHAND_OTEL_TRACES_SAMPLER_ARG` | `1.0` | Ratio in `[0,1]` (always ParentBased, including 0 and 1) |
| `MARKHAND_OTEL_METRICS_ENABLED` | `true` | In-process allowlisted metrics |
| `MARKHAND_OTEL_DISABLE_NETWORK` | `false` | Forced `true` under `MARKHAND_PROFILE=test` |
| `MARKHAND_OTEL_CAPTURE_IN_MEMORY` | `false` | Explicit test capture only; forbidden in prod; `exporter=none` never installs unbounded in-memory exporters by default |

Production fails closed on misconfig (otlp without endpoint, http remote endpoint,
or `DISABLE_NETWORK=true` with otlp). Dev collector: `deploy/dev/otel-collector.yaml`.
Dashboards/alerts are O02 — not configured here.

Run a config-only check:

```bash
cargo run -p fileconv-server -- --check-config
cargo run -p fileconv-server --bin fileconv-worker -- --check-config
```
