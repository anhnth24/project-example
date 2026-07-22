# Observability and audit conventions

Observability diagnoses system behavior; audit records security/business actions.
Neither channel may contain document text, prompt, PII, token, API key, signed URL,
database URL or object-storage credential.

## Correlation fields

Propagate through request → service → job/outbox → worker/artifact:

- `request_id`: one inbound interaction (UUID; `X-Request-Id`);
- `trace_id` / `traceparent`: W3C Trace Context (`traceparent` in job payloads; OTel context on request/worker spans);
- `org_id`: authorized tenant identifier, never user input before validation;
- `actor_id`: authenticated user/service identity when applicable;
- `job_id`, `document_id`, `document_version_id`;
- `index_signature`: model/chunk/dimension/normalize signature.

Missing tenant/actor fields are omitted, never filled with `"unknown"` for authorization.
Async workers restore `request_id` / `traceparent` from ID-only job payload fields and open a
child span (`worker`) with OTel parent/link via `.instrument(span)` (never `.enter()` across await).

## Structured logs

Allowlist stable identifiers, operation, outcome, duration, bounded error code and
correlation fields. Redact keys containing password, secret, token, authorization,
cookie, database URL, signed URL, document content, prompt or PII. Do not log raw
request/response bodies. Canary fixtures (`CANARY_*`) must never appear in captured
logs/spans/audit metadata.

## Metrics

- Prefix `markhand_`; snake_case; unit suffix (`_seconds`, `_bytes`, `_total`) where
  applicable.
- Labels are bounded enums/service/profile/outcome. Never use org/user/document/job ID,
  URL, filename, path, query, prompt/model response or error message as a label.
- Allowlisted series (O01):
  - `markhand_api_request_duration_seconds` / `markhand_api_requests_total`
    (`route`, `method`, `status_class`)
  - `markhand_queue_depth` / `markhand_queue_oldest_age_seconds` (`queue`)
  - `markhand_conversion_duration_seconds` / `markhand_conversion_total`
    (`format`, `result`)
  - `markhand_embedding_duration_seconds` / `markhand_embedding_total` (`result`)
  - `markhand_retrieval_duration_seconds` / `markhand_retrieval_total`
    (`leg`, `result`)
  - `markhand_drift_total` (`kind`, `state`) /
    `markhand_reconcile_total` (`mode`, `result`)
  - `markhand_quota_decisions_total` (`decision`, `resource_kind`)
  - `markhand_job_transitions_total` (`job_type`, `transition`, `result`)
  - `markhand_auth_decisions_total` (`result`, `code`)
- Backup metrics are emitted only when backup code paths exist (none in O01).
- Histogram buckets and SLO thresholds belong to Phase 0 evidence / O02 dashboards.

## OpenTelemetry (optional)

Configured via `MARKHAND_OTEL_*` (see `docs/conventions/config-secrets.md`). Default
exporter is `none` (local tracing + in-process metrics; no unbounded in-memory test
exporters unless `MARKHAND_OTEL_CAPTURE_IN_MEMORY=true`). `otlp` requires an endpoint
and enables secure TLS transport for HTTPS collectors; production misconfig fails
closed. Sampler is ParentBased for every ratio including 0 and 1. Test profile never
dials a collector. Dev collector: `deploy/dev/otel-collector.yaml` (OTLP gRPC `:4317`).
No Grafana dashboards in O01.

## Audit envelope

Audit events are append-only (DB trigger forbids UPDATE/DELETE) and tenant-scoped (RLS):

```json
{
  "version": 1,
  "occurredAt": "RFC3339 UTC",
  "requestId": "uuid",
  "orgId": "uuid",
  "actorId": "uuid",
  "action": "document.delete",
  "targetType": "document",
  "targetId": "uuid",
  "outcome": "success|deny|error",
  "metadata": {}
}
```

Metadata is an allowlist of non-sensitive scalar identifiers/reasons (see
`telemetry::redact::AUDIT_METADATA_ALLOWLIST`). Never content, prompts, answers,
object keys, emails, or secrets. Durable writers: `services/audit.rs` + `db/audit.rs`.

### Common actions

| Action | When |
|---|---|
| `auth.login` / `auth.refresh` / `auth.logout` | Session lifecycle |
| `auth.deny` | Authz deny (middleware / session) |
| `document.upload` / `document.publish` / `document.reindex` | Catalog mutations |
| `document.delete` / `document.tombstone` | Destructive delete path |
| `quota.deny` | Admission denial |
| `reconcile.repair` | Drift repair intent/result |

## Validation

`fileconv-server::telemetry` supplies field propagation, metric-name/label checks,
redaction helpers, and optional OTLP init. Integration coverage lives in
`crates/server/tests/telemetry_audit.rs`.
