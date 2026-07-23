# Converter outbreak

## Detect

- Alert: `MarkhandConversionFailures`
- Query:

```promql
sum(rate(markhand_conversion_duration_seconds_count{outcome=~"failed|error"}[5m]))
/
clamp_min(sum(rate(markhand_conversion_duration_seconds_count[5m])), 1e-9)
```

- Worker signals: sandbox preflight failures, convert timeouts (truncated `last_error` only).

## Contain

```bash
docker compose -f deploy/compose.poc.yml --env-file deploy/.env stop worker-convert
# Leave quarantine objects; do not promote
```

## Recover

```bash
docker compose -f deploy/compose.poc.yml --env-file deploy/.env exec -T worker-convert \
  /usr/local/bin/fileconv-worker --sandbox-preflight
docker compose -f deploy/compose.poc.yml --env-file deploy/.env logs --tail=80 worker-convert \
  2>&1 | python3 deploy/scripts/redact_secrets.py
docker compose -f deploy/compose.poc.yml --env-file deploy/.env start worker-convert
docker compose -f deploy/compose.poc.yml --env-file deploy/.env ps worker-convert
```

1. Re-enable one convert worker; soak a synthetic corpus sample.
2. Resume remaining workers only after failure ratio drops.
3. Never attach original documents or OCR text dumps to incident notes.

## Verify

- Failure ratio `< 0.2` for ≥10m; `MarkhandConversionFailures` inactive.
- No host FS / egress violations in redacted worker logs.
