# Local development stack

PostgreSQL, Qdrant, MinIO, telemetry and **AITeamVN CPU embedding** (same runtime as
on-prem CPU production).

**Runbook:** [`../../docs/runbooks/local-development.md`](../../docs/runbooks/local-development.md)

## First time

```bash
make dev-init
make dev-up && make dev-health    # first run: model download (~15 min possible)
# optional: deploy/scripts/download-aiteamvn-embedding.sh
set -a && source deploy/dev/.env && set +a
deploy/scripts/bootstrap-server-role.sh
cargo run -p fileconv-server
deploy/scripts/seed-dev-all.sh --skip-init
```

Embedding: `http://127.0.0.1:8088/v1` · model `AITeamVN/Vietnamese_Embedding` · 1024-d.

CI fast path: `COMPOSE_PROFILES=mock` (8-dim stub).

Compose: [`compose.yml`](compose.yml) · Dockerfile: [`Dockerfile.embedding-cpu`](Dockerfile.embedding-cpu)
