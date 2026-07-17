# Observability and audit conventions

Observability diagnoses system behavior; audit records security/business actions.
Neither channel may contain document text, prompt, PII, token, API key, signed URL,
database URL or object-storage credential.

## Correlation fields

Propagate through request → service → job/outbox → worker/artifact:

- `request_id`: one inbound interaction;
- `trace_id`: OpenTelemetry trace identifier;
- `org_id`: authorized tenant identifier, never user input before validation;
- `actor_id`: authenticated user/service identity when applicable;
- `job_id`, `document_id`, `document_version_id`;
- `index_signature`: model/chunk/dimension/normalize signature.

Missing tenant/actor fields are omitted, never filled with `"unknown"` for authorization.

## Structured logs

Allowlist stable identifiers, operation, outcome, duration, bounded error code and
correlation fields. Redact keys containing password, secret, token, authorization,
cookie, database URL, signed URL, document content, prompt or PII. Do not log raw
request/response bodies.

## Metrics

- Prefix `markhand_`; snake_case; unit suffix (`_seconds`, `_bytes`, `_total`) where
  applicable.
- Labels are bounded enums/service/profile/outcome. Never use org/user/document/job ID,
  URL, filename, prompt/model response or error message as a label.
- Histogram buckets and SLO thresholds belong to Phase 0 evidence, not ad-hoc code.

## Audit envelope

Audit events are append-only and versioned:

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
  "outcome": "allowed",
  "metadata": {}
}
```

Metadata is an allowlist of non-sensitive scalar identifiers/reasons. Audit is not an
authorization cache; source ACL snapshots are provenance only.

## Validation

`fileconv-server::telemetry` supplies field propagation, metric-name/label checks and
redaction helpers. F-11 is in-memory contract only: durable audit, middleware, exporter,
dashboard and SIEM integration arrive in later phases.
