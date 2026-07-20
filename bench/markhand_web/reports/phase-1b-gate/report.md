# Phase-1B gate qualification

- Generated: `2026-07-20T00:44:24.980717Z`
- Git commit: `2395b2ba8c9249181d79529b86e4981d081d6331`
- Dirty at report time: `true`
- `targetMatch`: `false`
- Counts: pass `0`, fail `0`, pending `16`

> Numeric G0-SLO/G0-CAP/DR/soak gates require sustained real infrastructure with targetMatch=true; pending/null rows are expected in this sandbox.

## Evidence inputs

| evidence | path | present | targetMatch | target-valid | report id |
|---|---|---|---|---|---|
| `soak` | `bench/markhand_web/soak/summary.json` | `true` | `false` | `false` | `p1b-o05-mixed-load-soak` |
| `query_load` | `bench/markhand_web/query_load/summary.json` | `true` | `false` | `false` | `p0-10-query-load-smoke` |
| `ingest` | `bench/markhand_web/ingest/summary.json` | `true` | `false` | `false` | `p0-08-ingest-capacity` |
| `restore` | `bench/markhand_web/restore/summary.json` | `true` | `false` | `false` | `p0-10-restore-drill` |

## Gates

| gate id | metric | threshold | status | measured value | evidence | reason |
|---|---|---:|---|---:|---|---|
| `G0-ARCH-DECISIONS` | `approved_architecture_decisions` `min` | `>= 7` | `pending` | `null` | `none` | not part of the P1B-O05 numeric soak/query/ingest/restore evidence set |
| `G0-RET-RECALL-AT-5` | `recall_at_5` `min` | `>= 0.85` | `pending` | `null` | `none` | not part of the P1B-O05 numeric soak/query/ingest/restore evidence set |
| `G0-RET-BEST-MODEL-GAP` | `best_model_ndcg_gap` `max` | `<= 0.02` | `pending` | `null` | `none` | not part of the P1B-O05 numeric soak/query/ingest/restore evidence set |
| `G0-RET-VLLM-CUTOVER` | `onprem_embedding_cutover_ready` `min` | `>= 1.0` | `pending` | `null` | `none` | not part of the P1B-O05 numeric soak/query/ingest/restore evidence set |
| `G0-RET-TEMPORAL-ACCURACY` | `temporal_answer_accuracy` `min` | `>= 0.95` | `pending` | `null` | `none` | not part of the P1B-O05 numeric soak/query/ingest/restore evidence set |
| `G0-RET-CHANGE-ACCURACY` | `version_change_accuracy` `min` | `>= 0.95` | `pending` | `null` | `none` | not part of the P1B-O05 numeric soak/query/ingest/restore evidence set |
| `G0-RET-VERSION-CITATION-PRECISION` | `version_citation_precision` `min` | `>= 1.0` | `pending` | `null` | `none` | not part of the P1B-O05 numeric soak/query/ingest/restore evidence set |
| `G0-RET-VERSION-CITATION-RECALL` | `version_citation_recall` `min` | `>= 1.0` | `pending` | `null` | `none` | not part of the P1B-O05 numeric soak/query/ingest/restore evidence set |
| `G0-SEC-UPLOAD-DENIAL` | `blocked_attack_fixtures` `min` | `>= 1.0` | `pending` | `null` | `none` | not part of the P1B-O05 numeric soak/query/ingest/restore evidence set |
| `G0-CAP-INGEST-THROUGHPUT` | `ingest_documents_per_hour` `min` | `>= 1200` | `pending` | `null` | `none` | evidence is present but not target-valid (targetMatch=false or targetResultsValidForGate=false) |
| `G0-SLO-QUERY-P95` | `query_latency` `p95` | `<= 500` | `pending` | `null` | `none` | evidence is present but not target-valid (targetMatch=false or targetResultsValidForGate=false) |
| `G0-SLO-QUERY-P99` | `filtered_query_latency` `p99` | `<= 1000` | `pending` | `null` | `none` | evidence is present but not target-valid (targetMatch=false or targetResultsValidForGate=false) |
| `G0-DR-RPO` | `recovery_point` `max` | `<= 15` | `pending` | `null` | `none` | evidence is present but not target-valid (targetMatch=false or targetResultsValidForGate=false) |
| `G0-DR-QUERY-READY-RTO` | `query_ready_recovery_time` `max` | `<= 60` | `pending` | `null` | `none` | evidence is present but not target-valid (targetMatch=false or targetResultsValidForGate=false) |
| `G0-DR-FULL-VECTOR-RTO` | `full_vector_recovery_time` `max` | `<= 240` | `pending` | `null` | `none` | evidence is present but not target-valid (targetMatch=false or targetResultsValidForGate=false) |
| `G0-LIC-INVENTORY` | `approved_runtime_licenses` `min` | `== 1.0` | `pending` | `null` | `none` | not part of the P1B-O05 numeric soak/query/ingest/restore evidence set |

## Interpretation

- `pending` means no target-valid evidence was available for that gate.
- `measured value` is `null` unless the source evidence is target-valid.
- Synthetic, redacted, local, or sandbox evidence must not be used as a numeric
  G0-SLO/G0-CAP/DR/soak pass.
