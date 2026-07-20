# P0-01 approval — workload, hardware and gates

- Approved at: `2026-07-18T07:37:00Z`
- Workload approver: `product-owner`
- Environment approver: `infrastructure-owner`
- Profile: `on-prem-reference-v1` / Profile B

## Scale and load envelope

- 20 organizations, 10 collections per organization, 5,000 documents per collection.
- Average 10 pages per document.
- Maximum 1,000,000 vectors per organization; 20,000,000 aggregate vectors.
- Normal load: 20 concurrent queries, 300 ingested documents/hour, 30 deletes/hour.
- Peak load: 80 concurrent queries, 1,200 ingested documents/hour, 120 deletes/hour.
- Recovery: 2× load for 120 minutes; aggregate concurrent ingest target 8.
- Tenant distribution: Zipfian 80/20.
- Selected-model quality gap: at most 0.02 absolute nDCG below the best evaluated
  candidate.

## Reference hardware

- 32 physical/reference cores, 64 threads, 256 GB RAM.
- 4 TB NVMe with at least 100k random-read IOPS.
- One accelerator with 24 GB VRAM.
- 10 Gbps network; assumed 1 ms local-service latency.
- Ubuntu 22.04, x86_64.

## Approved gate thresholds

| Gate | Threshold |
|---|---:|
| Required architecture decisions | ≥ 7 |
| Retrieval Recall@5 | ≥ 0.85 |
| Blocked adversarial upload fixtures | 1.00 |
| Peak ingest throughput | ≥ 1,200 documents/hour |
| Query latency P95 | ≤ 500 ms |
| Filtered query latency P99 | ≤ 1,000 ms |
| Best-model nDCG gap | ≤ 0.02 |
| Temporal/as-of answer accuracy | ≥ 0.95 |
| Version-change answer accuracy | ≥ 0.95 |
| Immutable-version citation precision | 1.00 |
| Immutable-version citation recall | 1.00 |
| Recovery point objective | ≤ 15 minutes |
| Query-ready recovery time | ≤ 60 minutes |
| Full-vector recovery time | ≤ 240 minutes |
| Approved runtime licenses | 1.00 |

All failures block Phase 1B. Gate approval fixes the target; it does not claim a
measurement passed. Raw evidence must identify actual hardware and fixture checksums.

## Current-runner limitation

The implementation runner observed 8 CPU, approximately 47 GB RAM, approximately
196 GB free disk and no visible GPU. It may run validators and reduced smoke tests,
but it cannot produce target-hardware, GPU or 20M-vector acceptance evidence.

## Embedding runtime (2026-07-20)

Supersedes interim exception below. ADR 0005 Accepted: Markhand Web POC/1B uses
**AITeamVN local CPU embedding** (`local-neural`). GLM cloud is **Q&A only**.
Profile B GPU + vLLM remains required for production cutover
(`G0-RET-VLLM-CUTOVER`), not for Phase 1B unblock.

## Embedding interim exception (2026-07-18, superseded)

~~Product approved ADR 0004: P0-05 / early G0-RET quality evidence may use GLM cloud
embeddings on `glm-cloud-interim` for coding/POC/DEMO.~~ Superseded 2026-07-20 by
local AITeamVN path (ADR 0005). Profile B GPU + vLLM remains required for
production cutover (`G0-RET-VLLM-CUTOVER`), not for Phase 1B unblock.
