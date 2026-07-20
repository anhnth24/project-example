# LLM/GLM provider outage and extractive fallback

Use this when the configured OpenAI-compatible/GLM chat provider times out, returns
errors, or must be disabled. The Q&A path buffers provider output and falls back to
extractive answers when provider streaming fails.

## Detection

- Query SLO alerts on `/api/v1/ask` or `/api/v1/ask/stream`.
- Logs from `server` containing provider timeout/transport/status errors or
  fallback warnings such as `LLM provider unavailable; using extractive fallback.`
- User reports that ask responses include fallback warnings.
- Metrics: `markhand_retrieval_latency_seconds{stage="ask"|"ask_stream",outcome}` and
  `markhand_http_requests_total{route=~"/api/v1/ask|/api/v1/ask/stream",status}`.

There is no dedicated O01 provider-health metric yet.

## Triage

```bash
docker compose -f deploy/compose.poc.yml logs --since=30m server
curl -fsS http://127.0.0.1:${MARKHAND_POC_SERVER_PORT:-8787}/api/v1/health/ready
```

Check configuration without printing secret values:

- `FILECONV_LLM_PROVIDER` is `openai` or `openai-compatible`.
- `FILECONV_LLM_BASE_URL` points at the intended GLM-compatible endpoint.
- `FILECONV_LLM_MODEL` is approved for this environment.
- `FILECONV_LLM_API_KEY` is present in the runtime secret store.

## Contain

1. If the provider is slow or unsafe, remove or blank `FILECONV_LLM_API_KEY` and
   restart the server so Q&A uses extractive fallback.
2. Keep retrieval and citation validation enabled. Do not emit unvalidated provider
   text directly to users.
3. If fallback quality is insufficient for an incident, temporarily disable ask in
   the client or route users to search-only workflows.

## Recover

- Restore the provider endpoint or rotate the provider key through the secret store.
- Restart the server after configuration changes:

  ```bash
  docker compose -f deploy/compose.poc.yml up -d server
  ```

- Send a low-risk ask request with citations and confirm no fallback warning is
  returned.
- If an embedding runtime changed, follow
  [Vector index rebuild](vector-index-rebuild.md) before claiming retrieval quality.

## Verify

- `/api/v1/health/ready` remains `200`.
- `/api/v1/ask` and `/api/v1/ask/stream` latency returns under the P95/P99 SLO panels.
- Ask output includes only citations that resolve through the document citation API.
- No provider secrets appear in logs.
