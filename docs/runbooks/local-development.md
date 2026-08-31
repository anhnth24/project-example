# Local development environment

Hướng dẫn chạy Markhand Web backend + dependency stack trên máy dev. Chi tiết toolchain
và quality gate xem [`contributor-setup.md`](contributor-setup.md).

F-08 cung cấp stack CPU-only cho development — **không** dùng làm bằng chứng benchmark hay
production throughput. Stack gồm PostgreSQL, Qdrant, MinIO, OpenTelemetry Collector và
**embedding-cpu (AITeamVN trên CPU)** — dev mặc định (không cần key/egress); deployment
hiện tại dùng OpenRouter `qwen3-embedding-8b` (ADR 0016), AITeamVN giữ cho air-gapped và
là đường self-host khi có GPU; lần đầu chậm vì tải model HuggingFace.

## Trên `master` hiện chạy được gì?

| Thành phần | Trạng thái |
|---|---|
| Dev stack (PG/Qdrant/MinIO/AITeamVN embed) | ✅ |
| `fileconv-server` — health live/ready | ✅ |
| Auth, upload quarantine, jobs (API) | ✅ (cần bật auth nếu gọi route bảo vệ) |
| `fileconv-worker` convert | ✅ Windows/Linux — subprocess trực tiếp (dev); Linux/Docker POC vẫn có sandbox cách ly |
| Index/embedding worker | ✅ cùng host với API (Windows/Linux) |
| Upload → convert → index qua HTTP | ✅ accepted upload tạo job `convert` durable trong cùng transaction (saga); cần convert + index + embedding worker chạy riêng để pipeline hoàn tất |
| Search (`/api/v1/search`), Ask (`/api/v1/ask`, `/ask/stream`), web SPA | ✅ có trên `master`; câu trả lời sinh bởi LLM (ngoài extractive fallback mặc định) và production qualification vẫn phụ thuộc runtime/evidence đã cấu hình |

Desktop Tauri (`pnpm --dir app tauri dev`) và CLI `fileconv` vẫn chạy độc lập — xem
[`CLAUDE.md`](../../CLAUDE.md).

## Prerequisites

- Docker Engine + Compose v2
- Rust 1.88 (rustfmt/clippy), GNU Make, Bash, curl, Python 3
- Node 20+ + pnpm 10.33.3 (chỉ khi chạy `web/` hoặc desktop)
- **Convert worker:** chạy cùng host với API (Windows hoặc Linux). Dev mặc định subprocess trực tiếp (không sandbox). Linux POC/Docker production vẫn sandbox trừ khi `MARKHAND_CONVERTER_DISABLE_SANDBOX=1`.

### Windows / Linux cùng host

| Việc | Windows (PowerShell) | Linux / macOS |
|---|---|---|
| Docker stack | `docker compose -f deploy/dev/compose.yml up -d` | `make dev-up` |
| Load `.env` | Git Bash `source deploy/dev/.env` hoặc set biến thủ công | `set -a && source deploy/dev/.env && set +a` |
| `fileconv-server` | `cargo run -p fileconv-server` | Giống |
| Workers (convert/index/embedding) | ✅ cùng host | ✅ |
| `fileconv` cho convert | `cargo build -p fileconv-cli` + argv `.exe` | `cargo build --release -p fileconv-cli` |

Ví dụ `MARKHAND_CONVERTER_ARGV_JSON` trên Windows (đường dẫn tương đối OK):

```json
["./target/debug/fileconv.exe","one","{input}","--ocr-defer-dir","."]
```

Tắt sandbox cách ly trên Linux dev (tuỳ chọn, giống Windows): `MARKHAND_CONVERTER_DISABLE_SANDBOX=1`.

Full stack trong Docker (API + workers): `deploy/scripts/poc-up.sh` — xem [`deploy/README.md`](../../deploy/README.md).

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

> **Windows:** port `54329` có thể rơi vào dải Hyper-V excluded port range
> (`netsh interface ipv4 show excludedportrange protocol=tcp`) — container lên
> nhưng host không connect được. Đổi `MARKHAND_POSTGRES_PORT` trong `.env`
> (ví dụ `55432`) và cập nhật `MARKHAND_DATABASE_URL`/`MARKHAND_WORKER_DATABASE_URL` tương ứng.
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

**Mặc định dev — AITeamVN CPU (air-gapped / self-host tương lai):** Compose
`embedding-cpu` @ `:8088`. Pin P0-05 trong `.env.example` (1024-d, revision `dea33aa1…`).
Worker và server dùng cùng `MARKHAND_EMBEDDING_*`. Deployment cloud-allowed hiện dùng
OpenRouter `qwen/qwen3-embedding-8b` (`provider-cloud` + `MARKHAND_ALLOW_CLOUD_EMBEDDINGS=true`,
ADR 0016 — xem `deploy/dev/worker.env.example`).

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

### Convert worker (Windows / Linux)

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
runtime lỗi → job failed; lexical search (`/api/v1/search`, FTS) vẫn hoạt động độc lập với
embedding.

## E2E checklist

### A. Smoke HTTP (mọi OS)

1. `make dev-init && make dev-up && make dev-health`
2. `bootstrap-server-role.sh` → `cargo run -p fileconv-server` (migrations) → `deploy/scripts/seed-dev-all.sh --skip-init`
3. `make dev-print-defaults`
4. Restart server → `curl` health, login, upload (mục Verify)

### B. Pipeline workers (cùng host hoặc Docker POC)

1. Hoàn thành A + `curl http://127.0.0.1:8088/health` (embedding-cpu ready)
2. Terminal riêng cho từng worker — convert, index (`MARKHAND_WORKER_KIND=index`), embedding
   (`MARKHAND_WORKER_KIND=embedding`) — xem mục Workers (`fileconv-worker`) để lấy env đầy đủ.
3. Upload một file **accepted** qua HTTP (curl mục Verify) hoặc qua Web SPA (`web/`). Upload
   accepted tạo job `convert` durable ngay trong transaction đăng ký document/version (saga
   `run_upload_saga`) — không cần enqueue thủ công.
4. Theo dõi job bằng `jobId` trong response upload:

   ```bash
   JOB_ID=<jobId lấy từ response upload>

   curl -sS http://127.0.0.1:8787/api/v1/jobs/$JOB_ID \
     -H "Authorization: Bearer $TOKEN"

   # hoặc theo dõi realtime qua SSE
   curl -sS -N http://127.0.0.1:8787/api/v1/jobs/$JOB_ID/events \
     -H "Authorization: Bearer $TOKEN"
   ```

   Convert worker hoàn tất → promotion (chốt version/markdown artifact hiện tại) ghi outbox
   event `document.index_requested`; trong setup convert/index/embedding đang mô tả ở đây,
   chính **index worker** tự relay outbox này thành job `index` trong claim-loop của nó
   (không phải convert worker tạo job `index` trực tiếp — nếu index worker không chạy,
   event outbox nằm chờ, chưa có job `index`) → job `index` hoàn tất tạo job
   `embedding_batch`; embedding worker upsert Qdrant (chi tiết ở mục Workers). Delete worker
   và reconcile worker dùng chung relay/sink này (`IndexingOutboxSink`/`OutboxJobSink`) để
   relay cả event `index` và `delete`, nằm ngoài walkthrough convert/index/embedding này.

5. Verify khả năng tìm kiếm sau khi index/embedding xong: `POST /api/v1/search` (mục Verify).
6. Verify hỏi-đáp: `POST /api/v1/ask` (mục Verify) — mặc định trả lời extractive
   (`offline_extractive`/`fallback_extractive`), không cần provider LLM nào. Chỉ cần cấu hình
   provider (`MARKHAND_CHAT_BASE_URL`/`MARKHAND_CHAT_API_KEY`/`MARKHAND_CHAT_MODEL` —
   hiện tại Qwen qua OpenRouter; alias legacy `MARKHAND_GLM_*` deprecated) khi muốn bật
   sinh câu trả lời bằng LLM thay cho extractive.

**Lưu ý:** quarantined/rejected upload **không** theo path convert accepted ở trên — upload
quarantined vẫn đăng ký document/version nhưng job `convert` chỉ được tạo khi có
`doc.quarantine.review` approve (`approve_quarantined_upload`); upload rejected không lưu
document/job nào.

7. (Tuỳ chọn) Integration test worker/indexing thay vì chạy worker thủ công:

```bash
# Cần stack + MARKHAND_TEST_* — xem crates/server/README.md
cargo test -p fileconv-server --test index_worker -- --ignored
```

8. Theo dõi job trong DB:

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
| `CloudRuntimeNotAllowed` | Dùng `local-neural`/`vllm-local` hoặc egress opt-in `MARKHAND_ALLOW_CLOUD_EMBEDDINGS=true` (mọi profile — ADR 0016) |
| Convert worker không tìm thấy `fileconv` | Build CLI: `cargo build -p fileconv-cli`; kiểm tra `MARKHAND_CONVERTER_ARGV_JSON` |
| Muốn sandbox cách ly trên Linux dev | Bỏ `MARKHAND_CONVERTER_DISABLE_SANDBOX` (mặc định sandbox trên Linux) |
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

Response accepted trả về `jobId` — theo dõi qua `GET /api/v1/jobs/{jobId}` hoặc
`GET /api/v1/jobs/{jobId}/events` (xem mục E2E checklist B).

### Search & Ask

Cần convert/index/embedding worker đã xử lý xong file upload (mục E2E checklist B) để có kết
quả khớp:

```bash
curl -sS -X POST http://127.0.0.1:8787/api/v1/search \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"query":"hello markhand","collectionIds":["55555555-5555-5555-5555-555555555501"]}'

curl -sS -X POST http://127.0.0.1:8787/api/v1/ask \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"question":"Tài liệu nói gì?","collectionIds":["55555555-5555-5555-5555-555555555501"]}'
```

`ask` mặc định trả lời extractive (`mode: offline_extractive`/`fallback_extractive`) khi chưa
cấu hình chat provider; cấu hình provider (`MARKHAND_CHAT_BASE_URL`/`MARKHAND_CHAT_API_KEY`/
`MARKHAND_CHAT_MODEL` — hiện tại Qwen qua OpenRouter; alias legacy `MARKHAND_GLM_*`
deprecated) chỉ cần khi muốn bật sinh câu trả lời bằng LLM.

## Failure and reset

- `make dev-health` sau mỗi restart stack.
- `make dev-reset` — xóa volume dev; không dùng credential/data production.

## Related

- [`contributor-setup.md`](contributor-setup.md) — CI gates, toolchain pin
- [`crates/server/README.md`](../../crates/server/README.md) — server/worker boundary
- [`deploy/scripts/README.md`](../../deploy/scripts/README.md) — init, seed, defaults
- [`bench/markhand_web/embedding/README.md`](../../bench/markhand_web/embedding/README.md) — quality-track model download
