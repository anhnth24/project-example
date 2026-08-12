# Test-server deployment

## Platform

Self-hosted Linux/amd64 Docker Compose. The UAT image serves the React SPA and
Rust API from one origin; five workers use PostgreSQL, Qdrant, MinIO, and —
per ADR 0016 — OpenRouter Qwen for vision OCR (`qwen/qwen3.7-flash`), cloud
embedding (`qwen/qwen3-embedding-8b`, explicit egress opt-in), and grounded
chat (OpenAI-compatible `MARKHAND_CHAT_*`). The local AITeamVN CPU embedding
service remains the air-gapped profile and the future self-host path once GPU
capacity exists.

This is a `dev`-profile, private-network test deployment. Direct HTTP is allowed
only for sanitized UAT documents and test identities. Add a reviewed TLS edge
before using customer data.

## Capacity boundary

- Corpus: 10–20 GB raw PDF/Office/image/text; audio is excluded.
- Seed organization: 30 GiB source-plus-derived quota, 10,000 documents.
- Stop ingestion when the host filesystem reaches 70% usage.
- Measure object, derived, chunk, vector, queue, token, and latency growth at
  5 GiB and again near 15–20 GiB.
- Qdrant starts with a 6 GiB memory limit for UAT. Qualify on-disk vectors before
  continuing if measured growth projects beyond that limit.

## Required untracked configuration

Create `deploy/.env` from `deploy/.env.example`, keep it mode `0600`, and set:

- `MARKHAND_COMPOSE_PROJECT=markhand-test`
- `MARKHAND_PROFILE=dev`
- `MARKHAND_API_BIND_HOST=<private-interface-ip>`
- matching `MARKHAND_AUTH_ISSUER`
- unique PostgreSQL, MinIO, JWT, and embedding credentials
- the OpenRouter cloud-embedding block (`qwen/qwen3-embedding-8b`,
  `MARKHAND_EMBEDDING_RUNTIME_PATH=provider-cloud`,
  `MARKHAND_ALLOW_CLOUD_EMBEDDINGS=true`, `MARKHAND_EMBEDDING_NORMALIZE=client`,
  `MARKHAND_EMBEDDING_SEND_DIMENSIONS=true`, pinned revision/dimensions and the
  matching `MARKHAND_INDEX_SIGNATURE`) — air-gapped deployments use
  `COMPOSE_PROFILES=aiteamvn` with the pinned AITeamVN block instead
- `MARKHAND_OCR_API_KEY` (vision OCR worker stage; base URL/model default to
  OpenRouter `qwen/qwen3.7-flash`)
- `MARKHAND_CHAT_BASE_URL`, `MARKHAND_CHAT_API_KEY`, and `MARKHAND_CHAT_MODEL`
  for grounded chat (currently Qwen via OpenRouter; a self-hosted
  OpenAI-compatible endpoint slots in unchanged once GPU capacity exists;
  legacy `MARKHAND_GLM_*` aliases are still read but deprecated)
- `MARKHAND_CHAT_PIN_HOSTNAME`/`MARKHAND_CHAT_HOST_IPV4` only when the chosen
  chat host resolves to unreachable AAAA records from the Docker bridge
- optionally `MARKHAND_QA_ALLOW_UNVERIFIED_LLM=1` only while UAT explicitly
  evaluates provider output; responses remain labelled `llm_unverified` with a
  warning because structured entailment is unavailable
- `MARKHAND_POC_MAX_STORAGE_BYTES=32212254720`
- `MARKHAND_QDRANT_MEM_LIMIT=6g`

Never commit the environment file, model weights, credentials, customer
documents, or secret-bearing logs.

Air-gapped only: keep AITeamVN at batch size 16 initially. During a measured
backlog ingest, `MARKHAND_EMBEDDING_CPUS` may be raised as high as `8.0` on the
24-CPU test host, then returned to the normal limit after the queue drains.

## Deploy

Use a clean release worktree at a reviewed commit. Before the first deployment,
verify at least 45 GiB free and that `psql`, `pg_dump`, `mc`, and `python3` are
available for the signed backup pipeline.

```bash
deploy/scripts/poc-isolation-smoke.sh
deploy/scripts/poc-up.sh
deploy/scripts/poc-health.sh
```

`poc-up.sh` builds the API/SPA and worker images, runs migrations, applies the
optional seeded-org quota override, starts dependencies and workers, and checks
readiness. (Air-gapped profile: the first AITeamVN boot downloads the pinned
model into the persistent embedding cache volume.)

Migration 0011 creates `admin@poc.example` without a password. Generate a random
test password on a trusted workstation, store it in the team password manager,
hash it with the repository's `dev-hash-password` Argon2 helper, and pipe only
the PHC hash to `deploy/scripts/poc-set-admin-password.sh` in the release
worktree. The script rejects plaintext/non-Argon2 input and does not print the
hash. Never deploy the documented `markhand-dev` development password.

## Acceptance

1. Verify `/`, `/api/v1/health/ready`, all five workers, and dependency health.
2. Run the real Playwright auth/library/upload suite from a workstation.
3. Upload a synthetic CASAN fixture, wait for `indexed`, then verify search and
   streamed grounded Q&A return a citation to that document.
4. Run one agent-browser exploratory pass across login/session, library,
   supported uploads, preview/download/actions, search, Q&A/chat history,
   graph, projects, members, and usage.
5. Store only sanitized command results, request IDs, screenshots, image IDs,
   and the deployed commit as evidence.

## Rollback

Keep the previous Compose project's volumes and a verified signed backup until
UAT acceptance. On failure, stop `markhand-test`, return the host port to the
previous project, and restart that project from its untouched checkout. Do not
delete old volumes during rollback.
