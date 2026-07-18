# OpenAI embedding catalog vs P0-05

## Research (before spend)

OpenAI currently exposes **exactly 3** embedding models via `/v1/embeddings`:

| Model | Dims | MIRACL avg (OpenAI) | Role |
|---|---:|---:|---|
| `text-embedding-3-large` | 3072 | 54.9 | current best |
| `text-embedding-3-small` | 1536 | 44.0 | current cheap |
| `text-embedding-ada-002` | 1536 | 31.4 | legacy |

No newer embedding SKU (v4 / GPT-embedding / etc.) appears in the official model catalog.
External VI evidence: OpenAI trails BGE-M3 / multilingual-e5 on Vietnamese retrieval benches
(~79% R@5 for 3-large vs ~90% hybrid BGE-M3 in one public writeup).

## Measured on Markhand golden (dense max-pool)

| Model | Dims | Recall@5 | nDCG@10 | Gate ≥0.85 |
|---|---:|---:|---:|---|
| `text-embedding-3-large` | 3072 | 0.7220 | 0.6424 | FAIL |
| `text-embedding-3-large` | 1536 | 0.6957 | 0.6283 | FAIL |
| `text-embedding-3-small` | 1536 | **0.7444** | **0.6625** | FAIL |
| `text-embedding-ada-002` | 1536 | 0.7794 | 0.6836 | FAIL |

## Verdict

**Không có model OpenAI nào đạt dense Recall@5 ≥ 0.85** trên corpus Phase 0.
Best-in-catalog on this corpus: `text-embedding-ada-002` @ 1536d → 0.7794.
P0-05 vẫn cần family on-prem (`bge-m3` / `multilingual-e5`), không đóng bằng OpenAI cloud.
