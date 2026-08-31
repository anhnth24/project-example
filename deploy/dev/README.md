# Local development stack

PostgreSQL, Qdrant, MinIO, telemetry and **AITeamVN CPU embedding** (dev default
— không cần key/egress; deployment hiện tại dùng OpenRouter `qwen3-embedding-8b`
theo ADR 0016, AITeamVN giữ cho air-gapped / self-host tương lai khi có GPU).

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

## Bật LLM cho dev (grounded ask, opt-in)

Mặc định `/api/v1/ask` là extractive-only (fail-closed vì chưa có structured
entailment verifier — xem `services/qa/mod.rs`). Để thử một provider
OpenAI-compatible thật trong dev (hiện tại: Qwen qua OpenRouter; local
self-host vLLM/Ollama dùng cùng contract):

```bash
# .env (không commit key thật):
MARKHAND_CHAT_BASE_URL=https://openrouter.ai/api/v1   # hoặc endpoint local :8089/v1
MARKHAND_CHAT_API_KEY=...                              # để trống nếu local không cần key
MARKHAND_CHAT_MODEL=qwen/qwen3.7-flash
# (alias legacy MARKHAND_GLM_* vẫn được đọc — deprecated)

# Dev-gate riêng — mặc định TẮT. Bật để answer LLM (sau khi qua citation/claim
# validation) được trả về thay vì luôn rớt extractive:
MARKHAND_QA_ALLOW_UNVERIFIED_LLM=1
```

Khi gate bật và provider trả lời hợp lệ, cả **JSON** `POST /ask` lẫn stream
`POST /ask/stream` trả `mode: "llm_unverified"` kèm warning cố định nói rõ
**chưa được xác thực bằng structured entailment — không phải câu trả lời
grounded** (stream gọi `complete()` buffered và giữ live-tail sống bằng
keepalive trong lúc chờ provider — xem `services/qa/ask_stream.rs`). Nếu câu
trả lời fail validation (citation bịa, sai delta, mâu thuẫn…) hệ thống vẫn rớt
về extractive như cũ. Tắt biến `MARKHAND_QA_ALLOW_UNVERIFIED_LLM` (hoặc không
set) để quay lại hành vi mặc định 100%.
