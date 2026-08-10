# Markhand Web benchmark assets

Phase 0 turns architecture assumptions into reviewable evidence. P0-01 defines:

- `workload-profile.yaml`: scale/load envelope and open product decisions;
- `environments/*.yaml`: hardware/environment profiles;
- `gates.yaml`: metric/threshold/command/environment/approver/failure registry;
- `schema/*.schema.json`: machine-readable contracts;
- `reports/environment-report.schema.json`: required benchmark evidence envelope.

The `.yaml` files use JSON-compatible YAML 1.2 so validation needs only Python stdlib.
`proposed` entries may contain explicit `null` values. `approved` workloads/gates may
not: approval means product/hardware owners supplied numeric values and approver.

```bash
make check-markhand-gates
```

## Approved reference profile

The authoritative decisions live in `workload-profile.yaml`. Product and
infrastructure owners approved the Profile B scale/load/hardware envelope and numeric
gate thresholds on 2026-07-18. Resolved decision records remain in the profile for
traceability.

`on-prem-reference` is a benchmark target, not a claim about the current runner.
Every measured report must include its actual environment fingerprint; smoke results
from smaller hardware cannot satisfy target-scale gates.

### POC/1B embedding runtime

ADR 0005 (Accepted 2026-07-20): Markhand Web uses **AITeamVN local CPU**
(`local-neural`, environment `local-cpu-quality`) for index/retrieval quality
evidence and dev stack. GLM cloud is **Q&A only** — not server embedding.
ADR 0004 (GLM cloud embedding interim) is superseded.

ADR 0016 (2026-08-10) retired the on-prem vLLM cutover gate
(`G0-RET-VLLM-CUTOVER` removed from `gates.yaml`): the alternative server
runtime is OpenRouter `qwen/qwen3-embedding-8b` (`provider-cloud`) behind the
explicit egress opt-in `MARKHAND_ALLOW_CLOUD_EMBEDDINGS=true`. It becomes the
default pin only after passing `G0-RET-RECALL-AT-5` / `G0-RET-BEST-MODEL-GAP`
on the golden corpus (DKP-02). AITeamVN `local-neural` remains the measured
baseline (Recall@5 0.9261) and the air-gapped profile.

Fixtures/corpus must be synthetic or de-identified, versioned and license-reviewed.
Large raw benchmark output remains a CI artifact; committed reports contain environment
fingerprint and checksums only.
