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
deploy/scripts/seed-poc-org.sh
```

The process applies migrations before listening. Seed the local POC organization only
after that first startup with `deploy/scripts/seed-poc-org.sh`.

Later code follows `route → service → repository/adapter`; business operations require
an explicit `OrgContext`.

Run `cargo run -p fileconv-server -- --check-config` to validate typed configuration
without starting a listener. See [`docs/conventions/config-secrets.md`](../../docs/conventions/config-secrets.md).

## Index and embedding workers

Run `fileconv-worker` separately for `MARKHAND_WORKER_KIND=index` and
`MARKHAND_WORKER_KIND=embedding`. The embedding worker has no hash fallback and
requires an approved local OpenAI-compatible runtime.

**Local dev (AITeamVN CPU embedding):** see
[`docs/runbooks/local-development.md`](../../docs/runbooks/local-development.md#embedding-runtime-index--embedding-workers).
Compose profile `aiteamvn` serves `http://127.0.0.1:8088/v1` (1024-d). CI uses profile
`mock`. Compute `MARKHAND_INDEX_SIGNATURE` with `python3 deploy/scripts/print-index-signature.py`.

```bash
export MARKHAND_EMBEDDING_BASE_URL=https://embedding.internal/v1
export MARKHAND_EMBEDDING_API_KEY=...
export MARKHAND_EMBEDDING_MODEL=BAAI/bge-m3
export MARKHAND_EMBEDDING_REVISION=<pinned-revision>
export MARKHAND_EMBEDDING_DIMENSIONS=1024
export MARKHAND_EMBEDDING_RUNTIME_PATH=vllm-local
# Required in prod; must equal the runtime-derived signature.
export MARKHAND_INDEX_SIGNATURE=<64-lowercase-hex>
```

Production and test profiles accept only local runtime paths (`vllm-local` or
`local-neural`). A cloud runtime is permitted solely for development when
`MARKHAND_ALLOW_CLOUD_EMBEDDINGS=true` is set explicitly; production rejects it
even if that flag is present.

### CI test strategy

`cargo test -p fileconv-server` is the required hermetic gate. Unit tests cover
the index-generation lifecycle (including post-cutover no-ops), empty-document
completion selection, active-generation document visibility, targeted-generation
collection/signature/state validation, pre-upsert lifecycle fencing, cancellation
compensation for index/embedding work, and embedding runtime policy.

`crates/server/tests/index_worker.rs` is explicitly ignored by the normal test
command because it needs PostgreSQL, MinIO, Qdrant, and that embedding runtime.
CI that runs it must provide `MARKHAND_TEST_DATABASE_URL`,
`MARKHAND_TEST_MINIO_ENDPOINT`, `MARKHAND_TEST_MINIO_ACCESS_KEY`,
`MARKHAND_TEST_MINIO_SECRET_KEY`, `MARKHAND_TEST_QDRANT_URL`, and the
`MARKHAND_EMBEDDING_*` variables above, then run:

```bash
cargo test -p fileconv-server --test index_worker -- --ignored
```
