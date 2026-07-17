# Phase 4 issues — Production hardening

Parent plan: [`../../../phase-4-production-hardening.md`](../../../phase-4-production-hardening.md)

<!-- roadmap-default-status: backlog -->

Mọi issue ở **Backlog**, blocked bởi P3-14. Threat-model notes có thể được ghi ở
phase trước nhưng không activate issue Phase 4 trước gate.

## Dependency

```text
P4-01 → P4-02 → P4-03 → P4-04 ──────────┐
P4-05 → P4-06 ───────────────────────────┤
P4-07 + P4-05 → P4-08 → P4-09 → P4-10 ─┤
                         └→ P4-11 ───────┤
P4-07..11 → P4-12 ──────────────────────┤
P4-01..12 → P4-13 ──────────────────────┤
                                         └→ P4-14
```

## P4-01 — OIDC Authorization Code + PKCE

- **Plan/files:** Discovery/auth/callback/PKCE/state/nonce/issuer/audience/JWKS/cache/
  rotation; web callback/error.
- **Depends:** Existing provider/session framework. **Acceptance/tests:** Identity
  `(issuer,subject)`, never email; invalid/replay/rotated-key/multi-issuer mock+staging.
- **Security/migration:** No code/token logs, separate identity linkage. **Out:** SAML.

## P4-02 — JIT provisioning và account linking

- **Plan/files:** Domain/claim allowlist, JIT policy, confirmation link, collision/
  recovery, federated identity migration.
- **Depends:** P4-01. **Acceptance/tests:** Ambiguous mapping fail closed; email change
  không transfer; spoof/duplicate/disabled-JIT/link/unlink tests.
- **Security/migration:** Unique immutable issuer+subject audit. **Out:** user policy DSL.

## P4-03 — Group/role sync và bounded deprovision

- **Plan/files:** Chọn SCIM/webhook/back-channel/periodic sync; map groups; cursor/
  status; suspend/revoke sessions/SSE/cache; max session age + alerts.
- **Depends:** P4-01/02 + IdP capability. **Acceptance/tests:** Measured revoke bound
  kể cả missed sync/outage; replay/order/group removal/in-flight tests.
- **Security:** Signed webhook/least SCIM token/audit. **Out:** document-driven identity.

## P4-04 — Session UI và break-glass

- **Plan/files:** Linked identities/session list, per-session/logout-all; MFA-protected
  restricted break-glass + alert/runbook.
- **Depends:** P4-01…03. **Acceptance/tests:** Revoked REST/SSE denied; IdP outage
  break-glass/rotation/race drill.
- **Security:** Emergency use always high-signal audit. **Out:** social login.

## P4-05 — Platform security/supply chain/secrets

- **Plan/files:** CSP/CSRF/CORS/TLS/proxy, SSRF/egress, sandbox, secret rotation,
  SBOM/dependency/image/license, audit tamper/retention.
- **Depends:** Deployment target; có thể song song OIDC. **Acceptance/tests:** No static
  secret; compromise drill; policy-enforced scans/security probes/rotation.
- **Security/migration:** Staged dual-key rotation. **Out:** custom crypto.

## P4-06 — Threat review/external pentest/remediation

- **Plan/files:** Update threat/data flow; scope tenancy/upload/parser/artifact/OIDC/
  egress/admin/deploy; remediate + regression + retest.
- **Depends:** P4-01…05 + stable staging. **Acceptance/tests:** Zero unresolved
  high/critical; risk được formally accepted phải có approver, compensating controls,
  expiry và retest date; external retest evidence.
- **Security:** Restricted report handling. **Out:** self-review thay pentest.

## P4-07 — HA/degraded modes/distributed limiting

- **Plan/files:** Stateless replicas, worker fairness, PG/MinIO/Qdrant HA, vLLM/GLM
  circuits, extractive mode, pause ingest, shared global limiter.
- **Depends:** SLA/deployment ADR. **Acceptance/tests:** Aggregate limits survive
  replica restart; authorized degraded behavior; fault/failover/load tests.
- **Security:** Auth/storage failure fail closed. **Out:** multi-site active-active.

## P4-08 — Reproducible production deployment

- **Plan/files:** One supported Helm/Kubernetes target, pinned images, ingress/TLS,
  secrets, network/egress, PV/backup, limits/autoscaling/PDB, topology profiles.
- **Depends:** P4-05/07. **Acceptance/tests:** Clean install; clear POC-vs-HA; lint/
  policy/install/upgrade/rollback/uninstall/pressure tests.
- **Security:** Least service accounts, no public data services. **Out:** Compose as prod.

## P4-09 — Backup/PITR/restore tooling

- **Plan/files:** Ordered/fenced PG+MinIO+Qdrant backup; signed manifest inventory/
  versions; reconcile readiness; rebuild path.
- **Depends:** P4-08 + RPO/RTO. **Acceptance/tests:** Detect missing/orphan/corrupt/
  incompatible before ready; scheduled restore/checksum/key tests.
- **Security:** Encrypt/isolate/audit backup credentials. **Out:** Qdrant-only backup.

## P4-10 — Clean DR/destructive failure drills

- **Plan/files:** Restore/rebuild, integrity/denial/golden suite; timed query-ready/full
  index; Qdrant/MinIO loss, corrupt migration, key compromise, tenant deletion.
- **Depends:** P4-08/09. **Acceptance/tests:** Approved RPO/RTO on clean environment,
  scenario evidence.
- **Security:** Isolated restored staging; rotate compromised keys. **Out:** paper drill.

## P4-11 — Migration/canary/rollback discipline

- **Plan/files:** Immutable lock; fresh/N-1/realistic duration; expand/backfill/dual/
  cutover/contract; versioned index/shadow/alias; payload compatibility; flags/canary.
- **Depends:** P4-08 + schema features. **Acceptance/tests:** Upgrade/rollback every
  supported release; mixed workers; long backfill; shadow quality; trigger simulation.
- **Security:** Isolation incident immediate stop/rollback. **Out:** destructive down migration.

## P4-12 — SRE dashboards/alerts/runbooks

- **Plan/files:** SLO/auth/queue/saturation/retrieval/drift/quota/restore/storage
  dashboards; ownership; executable runbooks.
- **Depends:** P4-07…11. **Acceptance/tests:** Controlled alerts; operators resolve
  jobs/parser/outage/quota/credential/tenant/disk game days.
- **Security:** No content/prompt/token/URL/PII telemetry. **Out:** staffing policy.

## P4-13 — Onboarding/help/accessibility/operator docs

- **Plan/files:** Create/join→collection→upload→Q&A wizard; Vietnamese help/admin
  checklist; user/admin/operator/security/API/license docs; WCAG fixes.
- **Depends:** Stable P4 UI/APIs. **Acceptance/tests:** New users finish unaided;
  WCAG 2.1 AA critical flows; usability/axe/keyboard/screen-reader/docs dry run.
- **Security:** Accurate privacy/egress/RBAC/quota guidance. **Out:** billing docs.

## P4-14 — Production go-live gate

- **Plan/files:** Full E2E/denial/load/soak/chaos/recovery/OIDC/pentest/accessibility/
  browser/upgrade/rollback/desktop/license matrix + signed checklist.
- **Depends:** P4-03…13. **Acceptance/tests:** Mandatory failures block release; zero
  unresolved high/critical (accepted risk phải đủ approver/control/expiry/retest);
  deprovision/RPO/RTO/HA/rollback/operator/onboarding gates all pass.
- **Security/migration:** Isolation incident luôn hard blocker. **Out:** post-P4 roadmap.

## Exit gate

Go-live cần OIDC/deprovision bound, zero unresolved high/critical findings (accepted
risk phải có approver/controls/expiry/retest), reproducible production install,
HA/distributed limits, clean DR, safe upgrade/rollback, executable operations,
accessible onboarding và toàn bộ denial/license/desktop regression evidence.
