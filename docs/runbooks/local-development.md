# Local development environment

Hướng dẫn chạy Markhand Web backend + dependency stack trên máy dev. Chi tiết toolchain
và quality gate xem [`contributor-setup.md`](contributor-setup.md).

F-08 cung cấp stack CPU-only cho development — **không** dùng làm bằng chứng benchmark hay
production. Stack gồm PostgreSQL, Qdrant, MinIO, OpenTelemetry Collector và mock embedding.
Các service dùng bridge riêng; port bind `127.0.0.1`; volume giữ data đến khi reset.

## Trên `master` hiện chạy được gì?

| Thành phần | Trạng thái |
|---|---|
| Dev stack (PG/Qdrant/MinIO/mock embed) | ✅ |
| `fileconv-server` — health live/ready | ✅ |
| Auth, upload quarantine, jobs (API) | ✅ (cần bật auth nếu gọi route bảo vệ) |
| `fileconv-worker` convert (Linux/WSL) | ✅ sandbox thật; Windows native fail-closed |
| Index/embedding worker | ✅ (`fileconv-worker` index + embedding) |
| Search/Q&A/web SPA | ⏳ Phase 1B R* / Phase 2 |

Desktop Tauri (`pnpm --dir app tauri dev`) và CLI `fileconv` vẫn chạy độc lập — xem
[`CLAUDE.md`](../../CLAUDE.md).

## Prerequisites

- Docker Engine + Compose v2
- Rust 1.88 (rustfmt/clippy), GNU Make, Bash, curl
- Node 20+ + pnpm 10.33.3 (chỉ khi chạy `web/` hoặc desktop)
- **Convert worker:** Linux hoặc WSL2 (namespace/cgroup sandbox); Windows native build server
  được nhưng conversion job sẽ báo sandbox unavailable

## Quick start — health check (~5 phút)

```bash
git clone <repository>
cd project-example

# 1. Toolchain (lần đầu)
make check-toolchain
make install

# 2. Dev stack
cp deploy/dev/.env.example deploy/dev/.env
make dev-up          # hoặc: deploy/scripts/up.sh
make dev-health      # deploy/scripts/health.sh

# 3. Load env vào shell hiện tại (bash/zsh)
set -a && source deploy/dev/.env && set +a
deploy/scripts/bootstrap-server-role.sh   # tạo role markhand_app (lần đầu)

# 4. API server
cargo run -p fileconv-server
# terminal khác:
curl --fail http://127.0.0.1:8787/api/v1/health/ready
```

Smoke một lệnh (build + start + seed POC org):

```bash
make dev-server-smoke   # deploy/scripts/server-smoke.sh
```

Dừng stack: `make dev-down`. Xóa volume local: `make dev-reset` (chỉ dev, không dùng
data thật).

## Cấu hình môi trường (`deploy/dev/.env`)

Copy từ [`deploy/dev/.env.example`](../../deploy/dev/.env.example). Các biến **bắt buộc**
cho server:

| Biến | Ví dụ local |
|---|---|
| `MARKHAND_PROFILE` | `dev` |
| `MARKHAND_BIND_ADDR` | `127.0.0.1:8787` |
| `MARKHAND_DATABASE_URL` | `postgres://markhand_app:markhand_app_dev_only@127.0.0.1:54329/markhand` |
| `MARKHAND_QDRANT_URL` | `http://127.0.0.1:6333` |
| `MARKHAND_MINIO_URL` | `http://127.0.0.1:9000` |
| `MARKHAND_MINIO_ACCESS_KEY` | `markhand` (khớp Compose) |
| `MARKHAND_MINIO_SECRET_KEY` | `markhand_dev_only` |

Kiểm tra config không start listener:

```bash
cargo run -p fileconv-server -- --check-config
```

Chi tiết secret/policy: [`docs/conventions/config-secrets.md`](../conventions/config-secrets.md).

### Auth (tùy chọn trên `dev`)

Profile `dev` **không bắt buộc** auth. Để thử login/upload, thêm vào `.env` (chỉ process
API, **không** set trên worker):

```bash
MARKHAND_AUTH_ISSUER=http://127.0.0.1:8787
MARKHAND_AUTH_AUDIENCE=markhand-dev
MARKHAND_AUTH_SIGNING_KEY=dev-only-signing-key-at-least-32-bytes
MARKHAND_AUTH_KID=dev-key-1
```

Sau khi server chạy migration lần đầu, seed org POC:

```bash
deploy/scripts/seed-poc-org.sh
```

Migration `0011` cũng seed org `poc` — script trên thêm membership owner tường minh.

## Convert worker (Linux / WSL)

Worker tách process, claim job `convert` từ PostgreSQL:

```bash
set -a && source deploy/dev/.env && set +a
# Worker: KHÔNG set MARKHAND_AUTH_* 
export MARKHAND_WORKER_ORG_ID=<org-uuid>
export MARKHAND_WORKER_USER_ID=<user-uuid>
export MARKHAND_WORKER_ID=dev-worker-1
export MARKHAND_CONVERTER_ARGV_JSON='["./target/release/fileconv","one","{input}"]'

cargo build --release -p fileconv-server
cargo run -p fileconv-server --bin fileconv-worker
```

Ví đầy đủ: [`crates/server/worker.env.example`](../../crates/server/worker.env.example).
Cần binary `fileconv` release trên PATH hoặc trong `MARKHAND_CONVERTER_ARGV_JSON`.

## Markhand Web shell (`web/`)

```bash
pnpm install
pnpm --dir web dev
```

Vite proxy `/api` → `http://127.0.0.1:8787`. Server phải đang chạy. OpenAPI contract:
`pnpm --dir web api:generate`.

## Endpoints dev stack

| Service | Local endpoint |
|---|---|
| PostgreSQL | `127.0.0.1:54329` |
| Qdrant | `http://127.0.0.1:6333` |
| MinIO API / console | `http://127.0.0.1:9000` / `http://127.0.0.1:9001` |
| OTLP gRPC / health | `127.0.0.1:4317` / `http://127.0.0.1:13133` |
| Mock embeddings | `http://127.0.0.1:8088/v1/embeddings` |
| Markhand API | `http://127.0.0.1:8787/api/v1` |

## GPU profile (tùy chọn)

Stack mặc định **không** start vLLM. Opt-in:

```bash
# set MARKHAND_VLLM_MODEL trong .env trước
docker compose -f deploy/dev/compose.yml --profile gpu up -d vllm
```

Model và performance evidence vẫn thuộc Phase 0 / cutover gate.

## Troubleshooting

| Triệu chứng | Gợi ý |
|---|---|
| `server requires MARKHAND_MINIO_ACCESS_KEY` | Thêm MinIO key/secret vào `.env` (xem example) |
| Readiness 503 | `make dev-health`; kiểm tra PG/Qdrant/MinIO |
| Migration fail / role | Chạy `deploy/scripts/bootstrap-server-role.sh` |
| Convert worker sandbox unavailable (Windows) | Dùng WSL2/Linux hoặc chỉ test API/upload path |
| `whisper-rs` build fail | cmake/clang/libstdc++; xem contributor-setup |
| Roadmap/API drift | `python3 scripts/build-roadmap.py`; `pnpm --dir web api:check` |

Logs: `docker compose -f deploy/dev/compose.yml logs <service>`.

## Verify bằng curl

Chạy sau khi `make dev-up` và `cargo run -p fileconv-server` (env đã `source`).

### Health (không cần auth)

```bash
# Liveness — process OK, không gọi dependency
curl -sS http://127.0.0.1:8787/api/v1/health/live | jq .

# Readiness — PG + Qdrant + MinIO (503 nếu stack chưa lên)
curl -sS -w "\nHTTP %{http_code}\n" http://127.0.0.1:8787/api/v1/health/ready | jq .

# Kỳ vọng: status "ok", requestId UUID; ready trả 200 khi stack healthy
```

### Auth (cần `MARKHAND_AUTH_*` trong `.env` + user có password)

Migration seed user `admin@poc.example` **không** có password — login curl chỉ work sau khi set
hash (xem `crates/server/tests/auth.rs` helper `seed_user`) hoặc dùng email/password test tự
tạo. Ví dụ khi đã có tài khoản:

```bash
# Login → lấy accessToken (jq cần cài riêng; bỏ jq nếu không có)
curl -sS -X POST http://127.0.0.1:8787/api/v1/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"email":"owner@example.test","password":"<your-dev-password>"}' | jq .

TOKEN=$(curl -sS -X POST http://127.0.0.1:8787/api/v1/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"email":"owner@example.test","password":"<your-dev-password>"}' \
  | jq -r .accessToken)

# Me — 401 nếu thiếu/không hợp lệ token
curl -sS http://127.0.0.1:8787/api/v1/auth/me \
  -H "Authorization: Bearer $TOKEN" | jq .
```

### Upload (auth + `doc.upload`)

```bash
printf 'hello markhand\n' > /tmp/markhand-verify.txt
curl -sS -X POST http://127.0.0.1:8787/api/v1/uploads \
  -H "Authorization: Bearer $TOKEN" \
  -F "file=@/tmp/markhand-verify.txt;filename=verify.txt" \
  -F 'collectionId=55555555-5555-5555-5555-555555555501' | jq .
# Kỳ vọng: disposition accepted|quarantined, objectKey, sha256
```

`collectionId` mặc định từ migration POC seed (`0011_expand_poc_seed.sql`).

### Route chưa có trên master

Search/Q&A (`/api/v1/search`, `/api/v1/ask`) — Phase 1B R*; curl sẽ 404 cho tới khi merge R04+.

## Failure and reset

- `deploy/scripts/health.sh` sau mỗi lần restart stack.
- `deploy/scripts/reset.sh` / `make dev-reset` chỉ cho dev local — không dùng credential
  production hay corpus nhạy cảm.

## Related

- [`contributor-setup.md`](contributor-setup.md) — CI gates, toolchain pin
- [`crates/server/README.md`](../../crates/server/README.md) — server/worker boundary
- [`deploy/dev/README.md`](../../deploy/dev/README.md) — compose overview
