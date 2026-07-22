# PG / Qdrant / MinIO / embedding outage

## Detect
- Readiness 503; `MarkhandDependencyDown`.

## Contain
- Keep API live endpoint up; readiness stays false.
- Stop mutation workers if PostgreSQL is unavailable.

## Recover
1. Restore the failed dependency.
2. For Qdrant-only loss, prefer rebuild from PG chunks (ADR 0012).
3. For MinIO loss, treat originals as potentially unrecoverable; inventory missing keys.

## Verify
- `/api/v1/health/ready` returns 200 only after reconcile-before-ready.
