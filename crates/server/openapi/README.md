# OpenAPI contract

`openapi.yaml` is the source of truth for Markhand Web public HTTP/SSE contracts.

```bash
pnpm --filter markhand-web api:generate
pnpm --filter markhand-web api:check
```

Fixtures must round-trip through `fileconv-server::api` types. Phase 1B OpenAPI covers
auth, upload, collections/documents/versions, citations, conflicts, jobs/events,
search, ask/stream, and health/readiness.
