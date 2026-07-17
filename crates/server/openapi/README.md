# OpenAPI contract

`openapi.yaml` is the source of truth for Markhand Web public HTTP/SSE contracts.

```bash
pnpm --filter markhand-web api:generate
pnpm --filter markhand-web api:check
```

Fixtures must round-trip through `fileconv-server::api` types. Phase F only supplies
sample health/SSE/error/pagination shapes; Phase 1B adds business routes.
