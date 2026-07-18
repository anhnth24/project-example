# fileconv-server

Markhand Web API and worker boundary. The first Phase 1B increment starts a real HTTP
server, applies checksum-verified PostgreSQL migrations, and exposes:

- `GET /api/v1/health/live` — process liveness;
- `GET /api/v1/health/ready` — PostgreSQL, Qdrant and MinIO readiness.

For a local run, start the real dependency stack and export endpoints from
`deploy/dev/.env.example`:

```bash
cp deploy/dev/.env.example deploy/dev/.env
make dev-up
set -a && source deploy/dev/.env && set +a
cargo run -p fileconv-server
curl --fail http://127.0.0.1:8787/api/v1/health/ready
```

The process applies migrations before listening. Seed the local POC organization only
after that first startup with `deploy/scripts/seed.sh`.

Later code follows `route → service → repository/adapter`; business operations require
an explicit `OrgContext`.

Run `cargo run -p fileconv-server -- --check-config` to validate typed configuration
without starting a listener. See [`docs/conventions/config-secrets.md`](../../docs/conventions/config-secrets.md).
