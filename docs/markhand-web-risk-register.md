# Markhand Web Phase 0 risk register

Status: active for Phase 0 close. P0-10 accepts architecture decisions and local
restore smoke only; high/critical Profile B items remain open where noted.

| ID | Severity | Risk | Owner | Disposition | Evidence / next action |
|---|---|---|---|---|---|
| R-P0-10-DR-01 | Critical | Real PostgreSQL/MinIO/Qdrant component-loss restore has not been measured on Profile B, so RPO/RTO targets are not proven. | operations-owner | Block production Phase 0 exit; run on-prem-reference restore drill. | ADR 0012; `bench/markhand_web/restore/summary.json` is smoke only with `targetMatch=false`. |
| R-P0-10-SCALE-01 | Critical | 20M aggregate vector query P99 and noisy-neighbor behavior are not measured on live Qdrant/PostgreSQL. | retrieval-owner, storage-owner | Block production scale claim; Profile B mixed-load run required. | ADR 0008, ADR 0009; P0-07 summary is synthetic only. |
| R-P0-10-CAP-01 | High | Peak ingest throughput and queue age/headroom are based on local converter smoke, not target workers/storage. | worker-owner | Block production capacity gate; rerun capacity harness on on-prem-reference. | P0-08 summary has `productionCapacityBlocked=true`. |
| R-P0-10-AUTH-01 | High | Auth/session lifecycle is accepted but not implemented or penetration-tested. | security-owner, server-owner | Block production auth rollout; implement ADR 0010 tests in Phase 1B. | ADR 0010. |
| R-P0-10-RLS-01 | High | RLS/OrgContext contract is accepted but not enforced by server migrations yet. | storage-owner, security-owner | Block multi-org production; Phase 1B must add policy and pool-leak tests. | ADR 0007; `phase-1b-single-org-poc.md`. |
| R-P0-10-MIG-01 | High | Model/index cutover from AITeamVN local CPU to on-prem vLLM may require full rebuild and rollback storage. | retrieval-owner, operations-owner | Block production embedding cutover until expand/cutover/contract runbook is tested. | ADR 0005, ADR 0006, ADR 0011. |
| R-P0-10-LIC-01 | High | Runtime license inventory passes for current required artifacts, but production image/package composition can change. | release-owner | Re-check before production packaging; block if any bundled runtime lacks approved redistribution. | `docs/markhand-web-runtime-license-inventory.json`; `scripts/check-runtime-license-inventory.py`. |
| R-P0-10-UPLOAD-01 | High | Upload sandbox and denial policy are smoke-tested, not proven in container runtime with malware scanning. | security-owner, worker-owner | Block production upload hardening; implement worker sandbox and scanner evidence. | `bench/markhand_web/security/summary.json`; P0-09 notes. |
| R-P0-10-QUEUE-01 | High | Recovery queue age under 2x normal load is simulated, not observed with real OCR/audio/vector workers. | worker-owner, operations-owner | Block production recovery target; include queue age in Profile B recovery drill. | `docs/markhand-web-sla-targets.md`; P0-08 queue simulation. |
| R-P0-10-MINIO-01 | High | MinIO originals are not reconstructable; object inventory drift can make restored metadata incomplete. | operations-owner, storage-owner | Require backup manifest and reconciliation before readiness. | ADR 0012; restore smoke emits placeholder checksums only. |

## Closure rule

P0-10 may be marked done when:

- required ADR decisions are accepted and machine-checked;
- runtime license inventory passes;
- upload security smoke remains closed;
- restore smoke emits recovery-order markers and honest blocked flags.

This does not close production Phase 0 exit. The critical/high Profile B blockers
above remain active until measured on `on-prem-reference` with `targetMatch=true`.
