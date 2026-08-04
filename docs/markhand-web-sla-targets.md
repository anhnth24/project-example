# Markhand Web SLA/SLO targets for Phase 0 gate

Status: Phase 0 targets accepted for planning. Production exit remains blocked
until Profile B measurements run on `on-prem-reference` with `targetMatch=true`.

## Scope

These targets apply to the Markhand Web Phase 1B service envelope:

- 20 orgs, 10 collections/org, 5000 documents/collection, 10 pages/document;
- up to 1M vectors/org and 20M aggregate vectors;
- normal load of 20 concurrent queries and 300 ingested documents/hour;
- peak load of 80 concurrent queries and 1200 ingested documents/hour;
- recovery load of 2x normal for 120 minutes.

## Targets

| Area | Metric | Target | Gate | Current evidence | Profile B status |
|---|---:|---:|---|---|---|
| Retrieval latency | Query P95 | <= 500 ms | `G0-SLO-QUERY-P95` | No gate evidence yet; query load smoke only | Blocked |
| Retrieval latency | Filtered query P99 | <= 1000 ms | `G0-SLO-QUERY-P99` | P0-07 topology smoke has `targetMatch=false`; not a pass | Blocked |
| Answer streaming | Time-to-first-token (TTFT) P95 | <= 1500 ms under normal load | Operational SLA (Phase 1B answer path) | Not measured on Profile B; desktop/local smoke only | Blocked |
| Availability | Service availability | >= 99.5% monthly for query path | Operational SLA | Not measured; spike lifecycle restart only | Blocked |
| Degraded mode | Authz-safe FTS/text fallback when vector index unavailable | Required; no cross-tenant leakage | Operational SLA / ADR 0007+0012 | Documented in ADRs; not Profile B proven | Blocked |
| Retrieval quality | Recall@5 | >= 0.85 | `G0-RET-RECALL-AT-5` | `bench/markhand_web/retrieval/summary.json` | Phase 1B quality track accepted |
| Temporal answers | Temporal accuracy | >= 0.95 | `G0-RET-TEMPORAL-ACCURACY` | `bench/markhand_web/retrieval/summary.json` | Phase 1B quality track accepted |
| Change answers | Change accuracy | >= 0.95 | `G0-RET-CHANGE-ACCURACY` | `bench/markhand_web/retrieval/summary.json` | Phase 1B quality track accepted |
| Version citations | Precision/recall | 1.0 / 1.0 | `G0-RET-VERSION-CITATION-*` | `bench/markhand_web/retrieval/summary.json` | Phase 1B quality track accepted |
| Ingest throughput | Peak documents/hour | >= 1200 | `G0-CAP-INGEST-THROUGHPUT` | P0-08 local-cpu smoke has `targetMatch=false`; not a pass | Blocked |
| Ingest throughput (POC) | Normal documents/hour | >= 300 | `G0-CAP-INGEST-THROUGHPUT-POC` | `bench/markhand_web/reports/phase-1b-gate/o05-soak.json` | Phase 1B POC scope on `poc-compose` |
| Queue age | Oldest ingest queue age under recovery load | <= 120 minutes and bounded | Capacity/recovery operational target | P0-08 deterministic simulation only | Blocked |
| DR RPO | Recovery point objective | <= 15 minutes | `G0-DR-RPO` | P0-10 restore smoke only; `targetMatch=false`; not a pass | Blocked |
| DR query-ready RTO | Query-ready recovery time | <= 60 minutes | `G0-DR-QUERY-READY-RTO` | P0-10 restore smoke only; `targetMatch=false`; not a pass | Blocked |
| DR full-vector RTO | Full vector rebuild/recovery time | <= 240 minutes | `G0-DR-FULL-VECTOR-RTO` | P0-10 restore smoke only; `targetMatch=false`; not a pass | Blocked |

## Phase 1C security/load qualification (G1C-SEC)

Status: **Repository design decision only** — thresholds below are machine-validated
in `bench/markhand_web/gates.yaml` and `scripts/check-markhand-gates.py` per
`docs/superpowers/plans/2026-07-31-phase1c-closure.md` Task 15. They apply to the
`phase1c-multi-org-poc` profile (2 orgs, mock embedding, dedicated worker DB URL).
**No qualifying run or external owner sign-off is implied by this table.** Measured
evidence remains `not_run` until Task 16 harness execution.

| Area | Metric | Target | Gate | Owner | Approver | Evidence status |
|---|---:|---:|---|---|---|---|
| Cross-tenant isolation | `cross_tenant_leakage_count` | == 0 | `G1C-SEC-LEAKAGE`, `G1C-SEC-QDRANT-FAIL-CLOSED` | security-owner | security-owner | not_run |
| ACL revoke latency | `membership_acl_revoke_max_ms` | <= 3000 ms | `G1C-SEC-REVOKE` | security-owner | operations-owner | not_run |
| Stale authorization | `post_commit_stale_authorizations` | == 0 | `G1C-SEC-ACL-CACHE`, `G1C-SEC-STALE-TOKENS` | security-owner | security-owner | not_run |
| Quota recovery | `quota_drift_after_recovery` | == 0 | `G1C-SEC-QUOTA-RECOVERY` | operations-owner | operations-owner | not_run |
| Noisy-neighbor fairness | `quiet_org_query_p95_ms` | <= 500 ms | `G1C-SEC-NOISY-NEIGHBOR` | operations-owner | operations-owner | not_run |
| Noisy-neighbor fairness | `starvation_events` | == 0 | `G1C-SEC-NOISY-NEIGHBOR` | operations-owner | operations-owner | not_run |
| Audit coverage | `admin_mutation_audit_coverage_ratio` | == 1.0 | `G1C-SEC-AUDIT-COVERAGE` | security-owner | security-owner | not_run |
| Worker least privilege | `worker_dedicated_role_verified` | == 1 | `G1C-SEC-WORKER-ROLE` | security-owner | operations-owner | not_run |
| Supply chain | `undispositioned_high_critical_count` | == 0 | `G1C-SEC-CONTAINER-VULNS` | security-owner | security-owner | not_run |

Phase 1C rollup gate `1C-13` remains a placeholder until all `G1C-SEC-*` rows pass with
`targetMatch=true` on the approved POC profile. This section does **not** claim Profile B
production scale, peak-tier capacity, or on-prem-reference SLO proof.

## Measurement rules

- Gate-valid SLO, capacity and DR measurements require `environmentId` =
  `on-prem-reference` and `targetMatch=true`.
- Local/offline harnesses may close Phase 0 implementation smoke only when they
  set honest flags such as `targetMatch=false`, `productionScaleBlocked=true`,
  `productionCapacityBlocked=true` or `profileBDrBlocked=true`.
- G0-DR evidence remains null in `bench/markhand_web/gates.yaml` until a real
  component-loss restore drill runs on Profile B.
- Query-ready RTO means PostgreSQL is restored, MinIO consistency checks pass,
  reconciliation has run, authorization is enforced, and either a valid vector
  snapshot is restored or a documented text/FTS fallback is enabled.
- Full-vector RTO means the active index generation is restored or rebuilt and
  verified against ADR 0006/0011 signature rules.

## Profile B blockers

The following targets block production Phase 0 exit and any Phase 1B scale claim:

1. `G0-SLO-QUERY-P95` and `G0-SLO-QUERY-P99` live mixed query/ingest/delete run.
2. `G0-CAP-INGEST-THROUGHPUT` on the on-prem-reference worker profile.
3. `G0-DR-RPO`, `G0-DR-QUERY-READY-RTO` and `G0-DR-FULL-VECTOR-RTO` component-loss
   restore drill with real PostgreSQL, MinIO and Qdrant artifacts.
4. On-prem vLLM cutover evidence for production embedding runtime.
5. TTFT P95 (`SLA-TTFT-P95`) under normal answer load on Profile B.
6. Monthly query-path availability (`SLA-AVAILABILITY`) measurement.
7. Authz-safe degraded-mode proof (`SLA-DEGRADED-MODE`) when the vector index is
   unavailable.
