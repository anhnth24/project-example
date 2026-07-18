# ADR 0004: Interim GLM cloud embedding; target remains on-prem vLLM

- Status: Accepted
- Date: 2026-07-18
- Decision key: `embedding-runtime-path`
- Owners: `product-owner`, `infrastructure-owner`, `retrieval-owner`
- Approver: product-owner (interim path); infrastructure-owner (on-prem cutover)
- Supersedes: N/A
- Related issues/PRs: `P0-05`, Phase 1B POC/DEMO

## Context

P0-05 originally required Profile B GPU + local `bge-m3` / multilingual-e5 via
vLLM before any embedding quality gate could unblock coding. The current runner
has no target GPU, while early Phase 0 → 1B work needs a usable neural embedding
path for programming, POC and DEMO.

Chat already permits GLM cloud with document-derived prompts. The remaining gap
is embedding ingest: cloud sends full chunk text at index build time, so the
corpus policy must be explicit.

## Decision

1. **Interim runtime (dev / POC / DEMO / Phase 1B):** use **GLM cloud embeddings**
   through the existing OpenAI-compatible client:
   - Provider: Zhipu/GLM (`openai-compatible`)
   - Default base URL: `https://open.bigmodel.cn/api/paas/v4`
     (international gateway `https://api.z.ai/api/paas/v4` allowed when configured)
   - Candidate models: `embedding-3` (primary), `embedding-2` (legacy compare)
   - Credentials: `FILECONV_EMBEDDING_API_KEY` (or in-memory desktop settings);
     never commit keys
   - Pin in index signature: provider, base URL host, model, revision/date,
     dimensions, normalize flag, truncation policy
2. **Target runtime (production / on-prem scale):** **self-host vLLM** on Profile B
   GPU with local `BAAI/bge-m3` and at least one multilingual-e5 family model.
   Cutover rebuilds the vector index; mixed generations are forbidden.
3. **P0-05 acceptance** may close on the interim GLM path for unblocking coding
   and Phase 1B POC/DEMO. VRAM/saturation/local-throughput evidence remains a
   **deferred cutover gate**, not a blocker for early delivery.
4. **Corpus policy for interim:** only synthetic / de-identified golden corpus
   (and explicitly approved fixtures) may be sent to GLM embedding APIs.
   Customer, restricted, or production content stays local until on-prem vLLM
   cutover (or a later classification review expands the allowlist).

## Consequences

- Phase 0/1B can proceed without waiting for GPU hardware.
- Index signatures must distinguish `glm-cloud` from `vllm-local` so a later
  cutover cannot silently mix dimensions or models.
- Cloud cost, rate limits and egress become operational concerns for DEMO only.
- Production claims about capacity (vectors/s, VRAM, noisy-neighbor) still require
  Profile B + vLLM measurements.
- Desktop and server share the same provider plan fields; desktop local-hash
  fallback remains available when the cloud key is absent.

## Alternatives considered

- **Wait for Profile B GPU before any P0-05 work:** rejects early POC/DEMO schedule.
- **Local hash 256-D only:** insufficient semantic quality for Vietnamese DEMO RAG.
- **Other cloud embedders (OpenAI/Gemini) as interim default:** rejected as default;
  GLM is the chosen interim provider for consistency with chat policy. Other
  providers remain available as desktop presets.
- **Skip neural embeddings until vLLM:** rejected; blocks retrieval/chunking tuning.

## Verification

- P0-05 interim report pins GLM provider/model/dimensions/normalize and records
  Recall@5/10, MRR, nDCG, API latency, failure rate on golden corpus.
- Index signature round-trip rejects mixing GLM and vLLM generations.
- Cutover checklist: provision vLLM → re-eval local models → rebuild index →
  retire GLM embedding credentials from server runtime.

## Exception lifecycle

| Field | Value |
|---|---|
| Exception | Cloud embedding egress for interim RAG |
| Owner | `product-owner` |
| Scope | Synthetic/de-identified corpus; dev/POC/DEMO/Phase 1B only |
| Expiry | On-prem vLLM cutover or Phase 4 production hardening, whichever first |
| Retest | Re-run embedding eval on Profile B GPU; confirm signature migration |
