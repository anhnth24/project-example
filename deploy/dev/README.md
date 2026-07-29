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

## Bật LLM cho dev (grounded ask, opt-in)

Mặc định `/api/v1/ask` là extractive-only (fail-closed vì chưa có structured
entailment verifier — xem `services/qa/mod.rs`). Để thử một provider GLM/
OpenAI-compatible thật trong dev:

```bash
# .env (không commit key thật):
MARKHAND_GLM_BASE_URL=http://127.0.0.1:8089/v1   # hoặc MARKHAND_CHAT_BASE_URL
MARKHAND_GLM_API_KEY=...                          # hoặc MARKHAND_CHAT_API_KEY; để trống nếu local không cần key
MARKHAND_GLM_MODEL=glm-4-flash                    # hoặc MARKHAND_CHAT_MODEL

# Dev-gate riêng — mặc định TẮT. Bật để answer LLM (sau khi qua citation/claim
# validation) được trả về thay vì luôn rớt extractive:
MARKHAND_QA_ALLOW_UNVERIFIED_LLM=1
```

Khi gate bật và provider trả lời hợp lệ, response có `mode: "llm_unverified"`
kèm warning cố định nói rõ **chưa được xác thực bằng structured entailment —
không phải câu trả lời grounded**. Nếu câu trả lời fail validation (citation
bịa, sai delta, mâu thuẫn…) hệ thống vẫn rớt về extractive như cũ. Tắt biến
`MARKHAND_QA_ALLOW_UNVERIFIED_LLM` (hoặc không set) để quay lại hành vi mặc
định 100%.
