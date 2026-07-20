# Local development environment

Hướng dẫn chạy Markhand Web backend + dependency stack trên máy dev. Chi tiết toolchain
và quality gate xem [`contributor-setup.md`](contributor-setup.md).

F-08 cung cấp stack CPU-only cho development — **không** dùng làm bằng chứng benchmark hay
production throughput. Stack gồm PostgreSQL, Qdrant, MinIO, OpenTelemetry Collector và
**embedding-cpu (AITeamVN trên CPU)** — cùng runtime dự kiến cho on-prem CPU production;
lần đầu chậm vì tải model HuggingFace.

## Trên `master` hiện chạy được gì?

| Thành phần | Trạng thái |
|---|---|
| Dev stack (PG/Qdrant/MinIO/AITeamVN embed) | ✅ |
| `fileconv-server` — health live/ready | ✅ |
| Auth, upload quarantine, jobs (API) | ✅ (cần bật auth nếu gọi route bảo vệ) |
| `fileconv-worker` convert (Linux/WSL) | ✅ sandbox thật; Windows native fail-closed |
| Index/embedding worker | ✅ (AITeamVN CPU @ `:8088`) |
| Upload → convert → index qua HTTP | ⏳ upload quarantine OK; enqueue document/job API chưa đủ |
| Search/Q&A/web SPA | ⏳ Phase 1B R* / Phase 2 |

Desktop Tauri (`pnpm --dir app tauri dev`) và CLI `fileconv` vẫn chạy độc lập — xem
[`CLAUDE.md`](../../CLAUDE.md).

## Prerequisites

- Docker Engine + Compose v2
- Rust 1.88 (rustfmt/clippy), GNU Make, Bash, curl, Python 3
- Node 20+ + pnpm 10.33.3 (chỉ khi chạy `web/` hoặc desktop)
- **Convert worker:** Linux hoặc WSL2 (namespace/cgroup sandbox)
- **Windows native:** build/run API + stack OK; convert worker fail-closed — dùng WSL2 cho worker

### Windows / WSL2

| Việc | Windows (PowerShell) | WSL2 (Ubuntu bash) |
|---|---|---|
| Docker stack | `make dev-up` hoặc `docker compose -f deploy/dev/compose.yml up -d` | Giống |
| Load `.env` | Không có `source` — dùng WSL hoặc set từng biến | `set -a && source deploy/dev/.env && set +a` |
| `fileconv-server` | `cargo run -p fileconv-server` | Giống |
| Workers (convert/index/embedding) | ❌ convert sandbox | ✅ khuyến nghị |
| curl verify | `curl.exe` hoặc WSL | `curl` |

Khuyến nghị: clone repo trong WSL (`\\wsl$\...`) và chạy stack + workers trong WSL; hoặc
chỉ test API/health trên Windows native.

## Quick start — health check (~5 phút)

```bash
git clone <repository>
cd project-example

make check-toolchain
make install

make dev-init
make dev-up && make dev-health   # lần đầu: đợi embedding-cpu tải model (có thể ~15 phút)

# Tuỳ chọn: tải model trước
# deploy/scripts/download-aiteamvn-embedding.sh

set -a && source deploy/dev/.env && set +a
deploy/scripts/bootstrap-server-role.sh

cargo run -p fileconv-server   # lần đầu: apply migrations, Ctrl+C khi ready
deploy/scripts/seed-dev-all.sh --skip-init
make dev-print-defaults        # in card: login, UUID, curl mẫu

cargo run -p fileconv-server   # chạy lại với auth bật sẵn
curl --fail http://127.0.0.1:8787/api/v1/health/ready
```

Smoke một lệnh (init env + stack + server + seed): `make dev-server-smoke`.

Dừng stack: `make dev-down`. Xóa volume: `make dev-reset` → lặp lại bootstrap, server
một lần, `seed-dev-all`.

## Khởi tạo env & seed dữ liệu dev

| Bước | Lệnh | Ghi chú |
|---|---|---|
| 1. File env | `make dev-init` | Tạo `deploy/dev/.env` + `worker.env` từ example (**không ghi đè** file có sẵn) |
| 2. Stack | `make dev-up` | PG, Qdrant, MinIO, **embedding-cpu (AITeamVN)** |
| 3. DB role | `bootstrap-server-role.sh` | Role `markhand_app` (sau `dev-reset`) |
| 4. Migrations | `cargo run -p fileconv-server` lần đầu | Tạo schema + seed POC migration `0011` |
| 5. Seed đầy đủ | `make dev-seed-all` | Org, password, metadata, in bảng defaults |
| 6. Xem defaults | `make dev-print-defaults` | Login, UUID, worker commands |

Chi tiết script: [`deploy/scripts/README.md`](../../deploy/scripts/README.md).

### Mặc định sau seed (dev-only)

| Mục | Giá trị |
|---|---|
| Password login | `markhand-dev` (override: `MARKHAND_DEV_PASSWORD`) |
| Admin | `admin@poc.example` — role admin (migration `0011`) |
| Owner | `owner@example.test` — role owner (`seed-poc-org`) |
| Org UUID | `11111111-1111-1111-1111-111111111111` |
| Collection upload | `55555555-5555-5555-5555-555555555501` |
| Auth | Bật sẵn trong `.env.example` (`MARKHAND_AUTH_*`) |
| Embedding | AITeamVN CPU `@ :8088`, signature pin trong `.env` |
| Metadata DB | `SELECT * FROM markhand_dev_seed;` |

```bash
# Chỉ set lại password (sau dev-reset + migrations)
make dev-seed-password
```

## Docker Compose (`deploy/dev/compose.yml`)

File compose: [`deploy/dev/compose.yml`](../../deploy/dev/compose.yml). Biến port/credential
lấy từ [`deploy/dev/.env`](../../deploy/dev/.env) (copy từ `.env.example`).

### Services (mặc định `make dev-up`)

| Service | Image | Host port | Vai trò |
|---|---|---|---|
| `postgres` | `postgres:18.4-bookworm` | `54329` | DB chính; healthcheck `pg_isready` |
| `qdrant` | `qdrant/qdrant:v1.18.2` | `6333` (HTTP), `6334` (gRPC) | Vector store cho index generation |
| `minio` | pinned MinIO | `9000` (API), `9001` (console) | Object storage quarantine + artifacts |
| `minio-init` | `minio/mc` | — | One-shot: tạo bucket `markhand-quarantine`, `markhand-documents`, `markhand-artifacts` |
| `otel` | OTel Collector | `4317` (gRPC), `13133` (health) | Telemetry dev (optional cho API) |
| **`embedding-cpu`** | `Dockerfile.embedding-cpu` | **`8088`** | **AITeamVN CPU** — profile `aiteamvn`, 1024-d L2 |
| `mock-embedding` | `python:3.12-alpine` | `8088` | Profile **`mock`** — stub 8-dim (CI) |

Profile **`aiteamvn`** (mặc định trong `.env`): `COMPOSE_PROFILES=aiteamvn`.

Profile **`mock`**: `COMPOSE_PROFILES=mock` — CI / smoke pipeline-only (không vector thật).

Profile **`gpu`** (opt-in):

| Service | Port | Ghi chú |
|---|---|---|
| `vllm` | `8000` | Cần NVIDIA + `MARKHAND_VLLM_MODEL` trong `.env` |

### Lifecycle

```bash
# First time / sau khi pull image mới
cp deploy/dev/.env.example deploy/dev/.env
make dev-up        # up -d + đợi minio-init + health + seed metadata

# Kiểm tra
make dev-health
docker compose -f deploy/dev/compose.yml ps

# Logs
docker compose -f deploy/dev/compose.yml logs -f embedding-cpu

# Prefetch weights (optional)
deploy/scripts/download-aiteamvn-embedding.sh

# Dừng (giữ volume)
make dev-down

# Reset toàn bộ data local (PG/Qdrant/MinIO volumes)
make dev-reset     # down --volumes rồi up lại
```

Sau `dev-reset`, chạy lại `bootstrap-server-role.sh` và start server (migration chạy lại).

Volume `embedding_model_cache` giữ weights HuggingFace (reset xóa cùng `make dev-reset`).

Server: [`deploy/scripts/aiteamvn-embedding-server.py`](../../deploy/scripts/aiteamvn-embedding-server.py)
— L2 normalize giống bench P0-05. Profile **`mock`**: [`mock-embedding.py`](../../deploy/scripts/mock-embedding.py).

## Cấu hình môi trường (`deploy/dev/.env`)

Tạo bằng `make dev-init` (copy từ [`deploy/dev/.env.example`](../../deploy/dev/.env.example)).
Example đã gồm **auth**, **AITeamVN embedding**, và **`MARKHAND_INDEX_SIGNATURE`** pin sẵn.

### Server (bắt buộc)

| Biến | Ví dụ local |
|---|---|
| `MARKHAND_PROFILE` | `dev` |
| `MARKHAND_BIND_ADDR` | `127.0.0.1:8787` |
| `MARKHAND_DATABASE_URL` | `postgres://markhand_app:markhand_app_dev_only@127.0.0.1:54329/markhand` |
| `MARKHAND_QDRANT_URL` | `http://127.0.0.1:6333` |
| `MARKHAND_MINIO_URL` | `http://127.0.0.1:9000` |
| `MARKHAND_MINIO_ACCESS_KEY` / `SECRET_KEY` | `markhand` / `markhand_dev_only` |

```bash
cargo run -p fileconv-server -- --check-config
```

Chi tiết secret/policy: [`docs/conventions/config-secrets.md`](../conventions/config-secrets.md).

### Auth (bật sẵn trong `.env.example`)

Biến `MARKHAND_AUTH_*` có trong `.env` sau `make dev-init` — **chỉ process API**,
không set trên worker. Seed login:

```bash
make dev-seed-all    # hoặc make dev-seed-password nếu migrations đã chạy
```

| Email | Role | Nguồn |
|---|---|---|
| `admin@poc.example` | admin | migration `0011` |
| `owner@example.test` | owner | `seed-poc-org` |

Password mặc định: **`markhand-dev`**. Override: `MARKHAND_DEV_PASSWORD=...`.

Volume `embedding_model_cache` giữ weights HuggingFace giữa các lần restart.

### Embedding runtime (index + embedding workers)

**Mặc định — AITeamVN CPU (dev = on-prem CPU prod path):** Compose `embedding-cpu` @
`:8088`. Pin P0-05 trong `.env.example` (1024-d, revision `dea33aa1…`). Worker và server
dùng cùng `MARKHAND_EMBEDDING_*`.

**Profile `mock`:** stub 8-dim cho CI — set `COMPOSE_PROFILES=mock` và uncomment block mock
trong `.env.example`.

**Bench harness** (`run_embedding_eval.py`) vẫn dùng để đo Recall@5 / gate evidence — cùng
model nhưng offline, không phục vụ HTTP worker.

| Biến | AITeamVN dev (mặc định) |
|---|---|
| `MARKHAND_EMBEDDING_BASE_URL` | `http://127.0.0.1:8088/v1` |
| `MARKHAND_EMBEDDING_API_KEY` | `dev-embedding-key` |
| `MARKHAND_EMBEDDING_MODEL` | `AITeamVN/Vietnamese_Embedding` |
| `MARKHAND_EMBEDDING_REVISION` | `dea33aa1ab339f38d66ae0a40e6c40e0a9249568` |
| `MARKHAND_EMBEDDING_DIMENSIONS` | `1024` |
| `MARKHAND_EMBEDDING_RUNTIME_PATH` | `local-neural` |
| `MARKHAND_INDEX_SIGNATURE` | `ca03085c…f65ae97c` (pin sẵn) |

Tính signature sau khi đổi bất kỳ biến embedding nào:

```bash
set -a && source deploy/dev/.env && set +a
python3 deploy/scripts/print-index-signature.py
# AITeamVN mặc định → ca03085c08f4c01d391ac973192815c944892f6e74b52e7bf4e1f135f65ae97c
```

Gắn vào `.env` nếu muốn pin generation giống prod: `MARKHAND_INDEX_SIGNATURE=<hex>`.

## Workers (`fileconv-worker`)

Worker tách process; **không** nhận `MARKHAND_AUTH_*`. Ví đầy đủ:
[`deploy/dev/worker.env.example`](../../deploy/dev/worker.env.example).

POC UUID (migration `0011`):

| Entity | UUID |
|---|---|
| Org | `11111111-1111-1111-1111-111111111111` |
| User (admin) | `22222222-2222-2222-2222-222222222201` |
| Collection | `55555555-5555-5555-5555-555555555501` |

Chuẩn bị chung (mọi worker):

```bash
set -a && source deploy/dev/.env && set +a
set -a && source deploy/dev/worker.env && set +a   # sau make dev-init
export MARKHAND_WORKER_ORG_ID=11111111-1111-1111-1111-111111111111
export MARKHAND_WORKER_USER_ID=22222222-2222-2222-2222-222222222201
cargo build --release -p fileconv-server
cargo build --release -p fileconv-cli    # cho convert worker
```

### Convert worker (Linux / WSL)

```bash
export MARKHAND_WORKER_ID=dev-convert-1
# MARKHAND_WORKER_KIND=convert   # default
export MARKHAND_CONVERTER_ARGV_JSON='["./target/release/fileconv","one","{input}"]'
cargo run --release -p fileconv-server --bin fileconv-worker
```

### Index worker

Cần `MARKHAND_EMBEDDING_*` (mock hoặc runtime thật):

```bash
export MARKHAND_WORKER_KIND=index
export MARKHAND_WORKER_ID=dev-index-1
cargo run --release -p fileconv-server --bin fileconv-worker
```

### Embedding worker

Cùng `MARKHAND_EMBEDDING_*` với index worker; gọi mock @ `:8088` hoặc runtime đã cấu hình:

```bash
export MARKHAND_WORKER_KIND=embedding
export MARKHAND_WORKER_ID=dev-embedding-1
cargo run --release -p fileconv-server --bin fileconv-worker
```

Index worker tạo job `embedding_batch`; embedding worker upsert Qdrant. Không có hash fallback —
runtime lỗi → job failed, lexical search vẫn độc lập (khi có R*).

## E2E checklist

### A. Smoke HTTP (mọi OS + WSL)

1. `make dev-init && make dev-up && make dev-health`
2. `bootstrap-server-role.sh` → `cargo run -p fileconv-server` (migrations) → `deploy/scripts/seed-dev-all.sh --skip-init`
3. `make dev-print-defaults`
4. Restart server → `curl` health, login, upload (mục Verify)

### B. Pipeline workers (Linux/WSL)

1. Hoàn thành A + `curl http://127.0.0.1:8088/health` (embedding-cpu ready)
2. Terminal: convert worker
3. Terminal: index worker (`MARKHAND_WORKER_KIND=index`)
4. Terminal: embedding worker (`MARKHAND_WORKER_KIND=embedding`)
5. Upload file qua curl (quarantine object trên MinIO)
6. **Lưu ý:** trên `master`, HTTP upload chưa enqueue job `convert`/`index` tự động — pipeline
   end-to-end qua API đang chờ issue document/job API (Phase 1B). Kiểm tra worker + indexing
   logic:

```bash
# Integration (cần stack + MARKHAND_TEST_* — xem crates/server/README.md)
cargo test -p fileconv-server --test index_worker -- --ignored
```

7. Theo dõi job trong DB (khi có job):

```bash
docker compose -f deploy/dev/compose.yml exec -T postgres psql \
  -U markhand -d markhand -c \
  "SELECT job_type, status, updated_at FROM jobs ORDER BY updated_at DESC LIMIT 10;"
```

## Markhand Web shell (`web/`)

```bash
pnpm install
pnpm --dir web dev
```

Vite proxy `/api` → `http://127.0.0.1:8787`. OpenAPI: `pnpm --dir web api:generate`.

## Endpoints dev stack

| Service | Local endpoint |
|---|---|
| PostgreSQL | `127.0.0.1:54329` |
| Qdrant | `http://127.0.0.1:6333` |
| MinIO API / console | `http://127.0.0.1:9000` / `http://127.0.0.1:9001` |
| OTLP gRPC / health | `127.0.0.1:4317` / `http://127.0.0.1:13133` |
| Embeddings (AITeamVN CPU) | `http://127.0.0.1:8088/v1/embeddings` |
| Markhand API | `http://127.0.0.1:8787/api/v1` |

## GPU profile (vLLM, tùy chọn)

```bash
# MARKHAND_VLLM_MODEL=... trong deploy/dev/.env
docker compose -f deploy/dev/compose.yml --profile gpu up -d vllm
```

Cập nhật `MARKHAND_EMBEDDING_*` trỏ tới vLLM (`http://127.0.0.1:8000/v1`, dimensions/model pin,
`MARKHAND_EMBEDDING_RUNTIME_PATH=vllm-local`), rồi `print-index-signature.py`. Evidence vẫn thuộc
Phase 0 / cutover gate.

## Troubleshooting

| Triệu chứng | Gợi ý |
|---|---|
| `server requires MARKHAND_MINIO_ACCESS_KEY` | Thêm MinIO key/secret vào `.env` |
| Readiness 503 | `make dev-health`; kiểm tra PG/Qdrant/MinIO |
| Migration fail / role | `deploy/scripts/bootstrap-server-role.sh` |
| `embedding runtime initialization failed` | `make dev-health`; xem logs `embedding-cpu`; prefetch script |
| Embedding 503 / loading | Model đang tải — đợi hoặc `download-aiteamvn-embedding.sh` |
| `SignatureMismatch` | Chạy `print-index-signature.py`, cập nhật `MARKHAND_INDEX_SIGNATURE` |
| `CloudRuntimeNotAllowed` | Dùng `local-neural`/`vllm-local` hoặc `MARKHAND_ALLOW_CLOUD_EMBEDDINGS=true` (dev) |
| Convert worker sandbox unavailable (Windows) | WSL2/Linux |
| Login 401 sau seed | Chạy `seed-dev-password.sh` sau khi server đã migrate |
| `whisper-rs` build fail | cmake/clang/libstdc++; xem contributor-setup |

Logs: `docker compose -f deploy/dev/compose.yml logs <service>`.

## Verify bằng curl

Sau `make dev-up`, server chạy, env đã `source`.

### Health (không auth)

```bash
curl -sS http://127.0.0.1:8787/api/v1/health/live
curl -sS -w "\nHTTP %{http_code}\n" http://127.0.0.1:8787/api/v1/health/ready
curl -sS http://127.0.0.1:8088/health
```

### Auth + upload

```bash
# Sau seed-dev-password.sh (password: markhand-dev)
TOKEN=$(curl -sS -X POST http://127.0.0.1:8787/api/v1/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"email":"admin@poc.example","password":"markhand-dev"}' \
  | python3 -c "import sys,json; print(json.load(sys.stdin)['accessToken'])")

curl -sS http://127.0.0.1:8787/api/v1/auth/me \
  -H "Authorization: Bearer $TOKEN"

printf 'hello markhand\n' > /tmp/markhand-verify.txt
curl -sS -X POST http://127.0.0.1:8787/api/v1/uploads \
  -H "Authorization: Bearer $TOKEN" \
  -F "file=@/tmp/markhand-verify.txt;filename=verify.txt" \
  -F 'collectionId=55555555-5555-5555-5555-555555555501'
```

### Route chưa có

Search/Q&A (`/api/v1/search`, `/api/v1/ask`) — Phase 1B R*.

## Failure and reset

- `make dev-health` sau mỗi restart stack.
- `make dev-reset` — xóa volume dev; không dùng credential/data production.

## Related

- [`contributor-setup.md`](contributor-setup.md) — CI gates, toolchain pin
- [`crates/server/README.md`](../../crates/server/README.md) — server/worker boundary
- [`deploy/scripts/README.md`](../../deploy/scripts/README.md) — init, seed, defaults
- [`bench/markhand_web/embedding/README.md`](../../bench/markhand_web/embedding/README.md) — quality-track model download
