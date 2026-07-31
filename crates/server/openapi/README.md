# OpenAPI contract

`openapi.yaml` is the source of truth for Markhand Web public HTTP/SSE contracts.

Built-in RBAC roles, active/reserved permissions, and default grants live in
`builtin-role-catalog.json`. OpenAPI exposes only the extension reference
`x-markhand-builtin-role-catalog: ./builtin-role-catalog.json` — it must not
embed a second grant matrix. Web presentation and PostgreSQL matrix tests
consume that same fixture; the database remains runtime authority.

```bash
pnpm --filter markhand-web api:generate
pnpm --filter markhand-web api:check
```

Fixtures must round-trip through `fileconv-server::api` types. Phase 1B OpenAPI covers
auth, upload, collections/documents/versions, citations, conflicts, jobs/events,
search, ask/stream, and health/readiness.
