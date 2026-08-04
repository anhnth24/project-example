<!-- generated-done-issue-plan: P0-05 -->
# P0-05 — Đánh giá embedding tiếng Việt

Date: 2026-08-04
Source issue: [#62](https://github.com/anhnth24/project-example/issues/62)
Catalog: [`backlog/phase-0/issues/README.md`](../markhand-web/backlog/phase-0/issues/README.md)
Phase plan: [`phase-0-discovery-and-gates.md`](../markhand-web/phase-0-discovery-and-gates.md)
Status: Done

## Objective

Chốt provider/model/revision/dimension/normalization đủ để lập
trình Phase 0→1B; giữ đường cắt sang on-prem vLLM.

## Context

- Phase: `0`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> closed 2026-07-20 on local AITeamVN CPU evidence (ADR 0005
> Accepted). Selected: `AITeamVN/Vietnamese_Embedding` Recall@5 **0.9261**, nDCG
> gap **0.0** vs BKAI comparator; `runtime_path=local-neural` on
> `local-cpu-quality`. GLM cloud embedding path (ADR 0004) superseded — GLM
> retained for Q&A only. GPU/vLLM capacity deferred (`G0-RET-VLLM-CUTOVER`,
> không chặn Phase 1B).

## Implementation plan

So hai family local trên golden corpus: `AITeamVN/Vietnamese_Embedding`
vs `bkai` bi-encoder; pin tokenizer/batch/truncation/dimensions/normalize; đo
theo category. Target (sau): so `bge-m3` và multilingual-e5 trên Profile B
GPU/vLLM. ~~Interim GLM cloud compare~~ superseded by local selection (journal
2026-07-20).

## Files/modules

`bench/markhand_web/embedding/`, `scripts/run_embedding_eval.py`,
`reports/embedding-evaluation.md`, `docs/adr/0005-vietnamese-embedding-model-quality.md`,
`docs/adr/0004-interim-glm-cloud-embedding.md` (superseded).

## Dependencies / blocks

Corpus + spike; target GPU không bắt buộc để đóng POC/1B.

## Acceptance criteria

≥2 local model families; cấu hình chọn đạt gate
quality và best-model-gap; config/signature immutable trong report;
`runtime_path=local-neural`.
≥2 model family local trên Profile B
GPU; có VRAM/throughput/saturation.

## Required tests / evidence

Recall/MRR/nDCG trên `embedding/results/summary.json`;
hybrid retrieval `retrieval/summary.json`; dev stack `embedding-cpu` @ `:8088`.

## Security and migration notes

Embedding on-prem; không gửi customer/restricted corpus
lên cloud cho index. GLM chỉ Q&A top-K. Index signature phân biệt
`local-neural` vs `vllm-local`; cắt sang vLLM = rebuild generation mới.

## Out of scope

Autoscaling; đổi desktop local-hash fallback mặc định.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Recorded commit/SHA references

- UNKNOWN — no completion/evidence commit is cited in the catalog status.

- GitHub sync-closed timestamp: `2026-07-20T10:35:09Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
