# Isolated single-org POC deployment scaffold

This scaffold is for a local, single-organization Markhand POC stack. It is not a
production deployment recipe.

> **NOT build-validated in this sandbox:** this Cursor Cloud environment has no
> Docker daemon, so `deploy/Dockerfile.server`, `deploy/Dockerfile.worker`, and
> `deploy/compose.poc.yml` were validated structurally only. Before relying on
> these artifacts, verify them on a host with Docker Engine:
>
> ```bash
> docker compose -f deploy/compose.poc.yml build
> docker compose -f deploy/compose.poc.yml up
> ```

## Run

From the repository root:

```bash
docker compose -f deploy/compose.poc.yml up --build
```

Stop and remove containers:

```bash
docker compose -f deploy/compose.poc.yml down
```

Remove named volumes as well:

```bash
docker compose -f deploy/compose.poc.yml down --volumes
```

## Services and ports

All externally published ports bind to `127.0.0.1` only.

| Service | Internal endpoint | Host endpoint | Purpose |
| --- | --- | --- | --- |
| `server` | `http://server:8787` | `http://127.0.0.1:${MARKHAND_POC_SERVER_PORT:-8787}` | API; runs migrations on startup |
| `postgres` | `postgres:5432` | `127.0.0.1:${MARKHAND_POC_POSTGRES_PORT:-15432}` | PostgreSQL 18.4 |
| `qdrant` | `http://qdrant:6333`, `qdrant:6334` | `127.0.0.1:${MARKHAND_POC_QDRANT_HTTP_PORT:-16333}`, `127.0.0.1:${MARKHAND_POC_QDRANT_GRPC_PORT:-16334}` | Vector store |
| `minio` | `http://minio:9000` | `127.0.0.1:${MARKHAND_POC_MINIO_API_PORT:-19000}` | S3-compatible object store |
| `minio` console | `http://minio:9001` | `127.0.0.1:${MARKHAND_POC_MINIO_CONSOLE_PORT:-19001}` | MinIO UI |

Health endpoints:

```bash
curl -fsS http://127.0.0.1:${MARKHAND_POC_SERVER_PORT:-8787}/api/v1/health/live
curl -fsS http://127.0.0.1:${MARKHAND_POC_SERVER_PORT:-8787}/api/v1/health/ready
curl -fsS http://127.0.0.1:${MARKHAND_POC_QDRANT_HTTP_PORT:-16333}/healthz
curl -fsS http://127.0.0.1:${MARKHAND_POC_MINIO_API_PORT:-19000}/minio/health/live
```

`/api/v1/health/ready` checks PostgreSQL connectivity, applied migrations,
Qdrant, MinIO, and the readiness fence. The O03 restore flow sets the fence to
`reconciling`/`restoring` during restore and back to `ready` after reconciliation.
Manual fence commands use the worker binary:

```bash
docker compose -f deploy/compose.poc.yml run --rm worker-reconcile readiness-fence reconciling "restore in progress"
docker compose -f deploy/compose.poc.yml run --rm worker-reconcile readiness-fence ready
```

## Binary and worker mapping

The Dockerfiles follow the actual Cargo package and bin names:

| Image | Cargo command | Runtime binary |
| --- | --- | --- |
| `server` | `cargo build --release -p fileconv-server --bin fileconv-server` | `/usr/local/bin/fileconv-server` |
| `worker-*` | `cargo build --release -p fileconv-server --bin fileconv-worker` | `/usr/local/bin/fileconv-worker` |
| `worker-*` convert helper | `cargo build --release -p fileconv-cli --bin fileconv` | `/usr/local/bin/fileconv` |

The compose file runs one worker service for each code-supported
`MARKHAND_WORKER_KIND`:

| Service | `MARKHAND_WORKER_KIND` | Notes |
| --- | --- | --- |
| `worker-convert` | `convert` | Uses `MARKHAND_CONVERTER_ARGV_JSON`, defaulting to `["/usr/local/bin/fileconv","one","{input}"]`. |
| `worker-index` | `index` | Uses local hash embeddings and `MARKHAND_INDEX_EMBEDDING_BATCH_SIZE`. |
| `worker-delete` | `delete` | Cleans object/vector state for delete jobs. |
| `worker-reconcile` | `reconcile` | Uses `MARKHAND_RECONCILE_MODE` (`repair` by default; code also accepts dry-run spellings). |

Workers require `MARKHAND_WORKER_ORG_ID` and `MARKHAND_WORKER_USER_ID`. The POC
defaults are placeholders for a single tenant context; override them to match the
seeded POC org/user IDs. Workers intentionally do **not** receive
`MARKHAND_AUTH_*` because `fileconv-worker` rejects API authentication settings.

## Configuration and secrets

`deploy/compose.poc.yml` passes the full server `MARKHAND_*` configuration surface
used by `crates/server/src/config.rs`, plus worker-only variables read by
`crates/server/src/bin/worker.rs`.

Change these before any non-local use:

- `MARKHAND_POSTGRES_PASSWORD` and the matching `MARKHAND_DATABASE_URL`
- `MARKHAND_MINIO_ACCESS_KEY` / `MARKHAND_MINIO_SECRET_KEY`
- `MARKHAND_AUTH_SIGNING_KEY` (also derives the download capability key in code)
- `MARKHAND_AUTH_ISSUER`, `MARKHAND_AUTH_AUDIENCE`, `MARKHAND_AUTH_KID`
- `MARKHAND_INDEX_SIGNATURE` if enforcing a pinned embedding runtime signature
- Any `FILECONV_LLM_*` settings if enabling cloud/compatible LLM Q&A

Isolation defaults:

- Single Compose project and a private bridge network named by the project.
- No cross-tenant posture: one worker org/user context is expected.
- Host ports bind to `127.0.0.1`.
- `MARKHAND_CORS_ALLOWED_ORIGINS` defaults empty and `MARKHAND_TRUSTED_PROXY`
  defaults false.
- Server and worker containers run as a non-root `markhand` user, with
  `read_only`, `cap_drop: ["ALL"]`, `no-new-privileges`, and `/tmp` tmpfs in
  compose.
- Images do not bake secrets; defaults are development-only Compose environment
  values marked `CHANGE IN PROD`.

## Native conversion runtime

`deploy/Dockerfile.server` installs only API runtime libraries
(`ca-certificates`, `libssl3`, `libstdc++6`, and `curl` for healthchecks).

`deploy/Dockerfile.worker` additionally installs conversion/OCR runtime
dependencies:

- `tesseract-ocr`
- `tesseract-ocr-vie`
- `tesseract-ocr-eng`
- `libstdc++6`

PDFium is not baked into the image. The worker image creates
`/opt/fileconv/pdfium/lib` and sets `FILECONV_PDFIUM_LIB` there; mount the output
of `bash bench/download_pdfium.sh` at that path for PDFium-backed rendering/OCR.
Without it, the converter falls back to the Rust/pdf-extract path where supported.

Optional runtime mounts:

- `FILECONV_PDFIUM_LIB=/opt/fileconv/pdfium/lib`
- `FILECONV_TESSDATA=/opt/fileconv/tessdata_best` after mounting `tessdata_best`
- `FILECONV_WHISPER_MODEL=/opt/fileconv/models/<model>.bin` after mounting a
  permitted Whisper model

Audio model weights, customer data, and real credentials must stay outside Git.
