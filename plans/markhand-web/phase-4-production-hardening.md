# Phase 4 — SSO, production hardening và onboarding

## Outcome

Đưa hệ thống từ sản phẩm nội bộ đã đủ tính năng thành deployment production có
federated identity, hardening, DR đã diễn tập, vận hành/audit và hướng dẫn đầy đủ.

## P4.1 — OIDC/SSO

- Authorization Code + PKCE.
- Validate issuer, audience, nonce, state, signature và key rotation.
- Identity key là `(issuer, subject)`, không dùng email làm khóa duy nhất.
- JIT provisioning theo policy.
- Org/domain mapping.
- Group/role sync và deprovision.
- Chốt cơ chế deprovision: SCIM, IdP webhook/back-channel logout, hoặc periodic sync
  có bounded interval + maximum session age; gate đo đúng revocation bound.
- Link local account với IdP có confirmation, tránh account takeover.
- Break-glass account và quy trình audit.
- Session revoke khi IdP/membership thay đổi.

UI:

- SSO button;
- callback/error;
- org picker;
- linked identity/account sessions;
- logout all sessions.

## P4.2 — Security hardening

- External penetration test và remediation.
- Threat model review sau implementation.
- Dependency/container/SBOM/license scanning.
- Secret manager, key rotation và compromised-token drill.
- CSP/CSRF/CORS/reverse-proxy/TLS review.
- SSRF/egress allowlist.
- Parser sandbox escape review.
- Audit tamper evidence, retention và export.
- Privacy/data-classification review cho GLM cloud.

Không go-live nếu còn high/critical finding chưa được giải quyết. Risk được chấp
nhận chính thức phải có approver, compensating controls, expiry và retest date mới
được xem là đã disposition; không được ghi nhận chung chung để bỏ qua gate.

## P4.3 — HA và degraded modes

Theo SLA/ADR đã chốt:

- API replicas và stateless routing;
- worker scaling/fair queue;
- PG HA/PITR;
- MinIO replication;
- Qdrant topology/snapshot;
- vLLM failover hoặc extractive degraded mode;
- GLM outage fallback;
- query vẫn phục vụ khi ingest tạm pause.

Mỗi dependency có readiness và circuit breaker phù hợp. Không trả stale unauthorized
cache khi auth/storage lỗi.

Rate limit phải dùng shared distributed state hoặc thiết kế global tương đương khi
có nhiều API replica; test tổng limit qua replica và sau restart, không dùng limiter
in-memory độc lập.

## P4.4 — Production deployment package

Chốt một deployment target được hỗ trợ (Kubernetes hoặc orchestrator on-prem tương
đương), cung cấp:

- reproducible manifests/chart và pinned images;
- reverse proxy/ingress + TLS;
- secret injection và rotation;
- namespace/network/egress policies;
- persistent volumes, backup hooks và resource limits;
- API/worker autoscaling;
- install, upgrade, rollback và uninstall validation;
- topology single-node POC và production HA được phân biệt rõ.

Compose POC Phase 1B không được quảng bá là production manifest.

## P4.5 — Backup/DR exercise

Thực hiện trên môi trường sạch:

1. Restore PG về recovery point.
2. Restore MinIO objects/versions.
3. Restore Qdrant snapshot hoặc rebuild từ PG chunks.
4. Verify backup manifest/app/migration/index compatibility.
5. Chạy integrity + denial + golden retrieval suite.
6. Đo service-queryable RTO và full-index RTO.

Diễn tập thêm:

- mất Qdrant;
- mất MinIO;
- corrupt migration/index;
- key compromise;
- accidental tenant delete.

## P4.6 — Migration và rollout discipline

- Immutable versioned migrations + lock.
- CI test fresh install, supported upgrades và realistic-data duration.
- Expand → backfill → dual read/write nếu cần → cutover → contract.
- Qdrant index versioned, shadow query, alias switch và giữ bản trước để rollback.
- Worker khai báo schema/job payload compatibility.
- API và worker canary độc lập.
- Feature flag cho format, cloud GLM, intelligence và index signature.

Rollout:

```text
dev → restored staging → internal org → allowlisted POC org → broader rollout
```

Định nghĩa rollback trigger trước release: error, latency, queue age, retrieval
regression, reconciliation drift hoặc bất kỳ isolation incident nào.

## P4.7 — Observability/SRE completion

Dashboard và alert:

- SLO burn;
- auth anomaly;
- queue age/capacity;
- converter/GPU saturation;
- retrieval quality/latency;
- reconciliation drift;
- quota leak;
- backup/restore-test age;
- disk/object/vector growth.

Runbook bắt buộc:

- stuck jobs/dead letters;
- parser outbreak;
- PG/Qdrant/MinIO/vLLM/GLM outage;
- rebuild/rollback model;
- quota discrepancy;
- credential rotation;
- tenant-access incident;
- disk exhaustion.

## P4.8 — Help và onboarding

Web:

- onboarding create/join org → collection → upload → first Q&A;
- help tiếng Việt về format/limit, citation, privacy, quota, RBAC;
- admin setup checklist;
- contextual empty/error states;
- searchable help;
- accessibility WCAG 2.1 AA cho critical flows.

Docs:

- user guide;
- org admin guide;
- operator install/upgrade/backup/restore;
- security/data-flow overview;
- API/OpenAPI;
- model/license inventory.

## P4.9 — Final validation

- Full E2E, denial, load, soak, chaos và recovery.
- OIDC mock + IdP staging.
- Pentest retest.
- Accessibility audit.
- Browser compatibility.
- Upgrade/rollback rehearsal.
- Desktop regression suite.
- License review, đặc biệt model audio/OCR.

## Gate

- OIDC, deprovision và break-glass flow pass.
- Zero unresolved high/critical security findings; mọi accepted risk có approver,
  controls, expiry và retest date.
- DR đạt RPO/RTO đã duyệt.
- HA/degraded modes hoạt động đúng.
- Upgrade và rollback rehearsal thành công.
- Operator có thể cài, backup, restore và xử lý incident bằng runbook.
- Người dùng mới hoàn thành upload → Q&A qua onboarding mà không cần hỗ trợ trực tiếp.

## Sau Phase 4

Chỉ mở roadmap mới dựa trên telemetry và nhu cầu thật, ví dụ custom roles, billing,
advanced workflow approval, server-side connectors/watch hoặc HA đa site. Không đưa
các hạng mục này ngược vào scope production đầu tiên.
