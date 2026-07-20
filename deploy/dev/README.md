# Local development stack

CPU-only PostgreSQL, Qdrant, MinIO, telemetry and mock embedding services for local
development.

**Hướng dẫn từng bước (quick start, env, worker, web, troubleshooting):**
[`../../docs/runbooks/local-development.md`](../../docs/runbooks/local-development.md)

Lệnh rút gọn:

```bash
cp deploy/dev/.env.example deploy/dev/.env
make dev-up && make dev-health
set -a && source deploy/dev/.env && set +a
deploy/scripts/bootstrap-server-role.sh
cargo run -p fileconv-server
```
