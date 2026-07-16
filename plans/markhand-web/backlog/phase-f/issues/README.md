# Phase F issues — Engineering foundation

Parent plan: [`../../../phase-f-engineering-foundation.md`](../../../phase-f-engineering-foundation.md)

<!-- roadmap-default-status: blocked -->

## Dependency

```text
F-01 → F-02 ─┬→ F-03
             ├→ F-04
             ├→ F-05
             ├→ F-06
             └→ F-07 → F-08
F-03..08 ───────→ F-10 → F-09
F-01 + F-06 + F-07 + F-09 → F-11
F-01..11 ─────────────────────→ F-12
```

Diagram là critical path rút gọn; trường `Dependencies/blocks` là authority.

## F-01 — Architecture boundaries và dependency rules

- **Status:** Ready.
- **Objective:** Khóa dependency direction và module responsibilities trước scaffold.
- **Implementation plan:** Viết architecture boundary ADR; define allowed/forbidden
  dependencies; route→service→repository; tenant context rule; browser/Tauri split;
  automated `cargo tree`/import checks. Bootstrap minimum CODEOWNERS, issue/PR
  template, Definition of Ready/Done và security-review triggers để govern chính
  Phase F; F-12 hoàn thiện và kiểm chứng workflow.
- **Files/modules:** `docs/adr/0001-web-boundaries.md` (new),
  `docs/conventions/dependencies.md` (new), `.github/CODEOWNERS`,
  `.github/{ISSUE_TEMPLATE,PULL_REQUEST_TEMPLATE}.md`, CI boundary scripts.
- **Dependencies/blocks:** Không; blocks F-02 và mọi crate/web implementation.
- **Acceptance criteria:** Core không framework/storage; knowledge pure mặc định;
  server không reverse-depend desktop; web không Tauri; vendor không dependency.
- **Required tests/evidence:** Positive/negative sample boundary checks trong CI;
  architecture diagram và approver.
- **Security/migration:** Tenant context bắt buộc ở repository rule; migration N/A.
- **Out of scope:** Storage trait tổng quát và business implementation.

## F-02 — Workspace và folder skeleton

- **Status:** Blocked bởi F-01.
- **Objective:** Tạo khung compile được cho knowledge/server/web/deploy/docs/bench.
- **Implementation plan:** Add workspace members với minimal libraries/binaries; module
  READMEs/ownership; Vite web shell; deploy/dev placeholders; không copy business
  logic. Chốt root pnpm workspace/lockfile policy cho `app/` + `web/`; pin Node,
  pnpm, task runner và Compose requirements; thêm bootstrap/version-check command.
- **Files/modules:** `Cargo.toml`, `crates/{knowledge,server}/`, `web/`, `deploy/dev/`,
  `docs/{adr,conventions,runbooks}/`, `bench/markhand_web/`.
- **Dependencies/blocks:** F-01; blocks coding/tooling/dev environment issues.
- **Acceptance criteria:** Cargo workspace và web build; server API/worker binaries start
  help/config validation only; no cyclic/forbidden deps; JS workspace/lockfile policy
  và host tool versions được máy kiểm tra.
- **Required tests/evidence:** `cargo metadata/check`, bootstrap/version check,
  `pnpm install --frozen-lockfile`, app+web build, tree/import boundary.
- **Security/migration:** No credential/default public bind; no DB migration.
- **Out of scope:** Auth/schema/routes/jobs.

## F-03 — Rust coding và crate conventions

- **Status:** Blocked bởi F-02.
- **Objective:** Một chuẩn Rust bắt buộc cho core/knowledge/server/workers.
- **Implementation plan:** Rustfmt/clippy policy; error/context; async vs blocking;
  cancellation/timeouts; panic/unwrap/unsafe/public docs; naming/module visibility.
- **Files/modules:** `rustfmt.toml`, `clippy.toml` nếu cần,
  `docs/conventions/rust.md`, root lint task, CI.
- **Dependencies/blocks:** F-02; blocks Rust feature issues.
- **Acceptance criteria:** Convention có enforceable rule + justified exceptions;
  existing code có migration plan thay vì bật deny phá toàn repo ngay.
- **Required tests/evidence:** Format check, clippy selected warnings-as-errors,
  forbidden-pattern baseline/delta.
- **Security/migration:** Request/worker path không panic; secret-safe errors; N/A schema.
- **Out of scope:** Refactor toàn bộ warning cũ trong cùng issue.

## F-04 — TypeScript/React conventions

- **Status:** Blocked bởi F-02.
- **Objective:** Chuẩn strict TS, component/hook/state và accessibility cho web.
- **Implementation plan:** TS strict policy; generated API immutable; naming/import
  boundaries; state ownership; loading/error/empty; abort cleanup; a11y checklist.
- **Files/modules:** `web/tsconfig*.json`, ESLint/Prettier config,
  `docs/conventions/typescript-react.md`, web test setup.
- **Dependencies/blocks:** F-02; blocks Phase 2 implementation.
- **Acceptance criteria:** No Tauri imports; generated code separated; hooks clean up
  requests/streams; component patterns documented.
- **Required tests/evidence:** Typecheck/lint/format/unit sample/a11y smoke.
- **Security/migration:** No token/content logging or unsafe HTML by default; N/A schema.
- **Out of scope:** Full design system và Phase 2 pages.

## F-05 — SQL/data/migration conventions

- **Status:** Blocked bởi F-01/F-02.
- **Objective:** Ngăn schema/tenant/migration conventions bị phát minh theo từng PR.
- **Implementation plan:** Naming/types/time/UUID/FK/check/index; `org_id`; transaction/
  locking/idempotency; immutable migration; expand/backfill/cutover/contract; rollback.
- **Files/modules:** `docs/conventions/sql-migrations.md`,
  `crates/server/migrations/README.md`, migration test harness skeleton.
- **Dependencies/blocks:** F-01/02; blocks Phase 1B schema.
- **Acceptance criteria:** Example migration/repository query hợp conventions; policy
  fresh/upgrade/mixed-version rõ.
- **Required tests/evidence:** Empty DB apply, migration checksum/immutability,
  rollback-compat sample.
- **Security/migration:** Tenant predicate/RLS review checklist bắt buộc.
- **Out of scope:** Business tables và RLS decision.

## F-06 — REST/OpenAPI/SSE/error conventions

- **Status:** Blocked bởi F-01/F-02.
- **Objective:** Contract thống nhất để backend/web không drift.
- **Implementation plan:** `/api/v1`; resources/pagination/idempotency; canonical error;
  date/UUID/enum/null; OpenAPI authority; SSE envelope/version/sequence/reconnect;
  deprecation policy.
- **Files/modules:** `docs/conventions/api.md`, `crates/server/openapi/`,
  sample DTO/error/SSE types và fixtures.
- **Dependencies/blocks:** F-01/02; blocks 1B routes và Phase 2 client.
- **Acceptance criteria:** Sample contract generate TS; error/SSE fixtures round-trip;
  compatibility rules có examples.
- **Required tests/evidence:** OpenAPI validation/snapshot, Rust↔TS fixture,
  SSE parser sequence sample.
- **Security/migration:** Errors không leak internal; SSE auth/revocation requirements;
  persisted migration N/A.
- **Out of scope:** Business endpoints.

## F-07 — Configuration, secrets và environment profiles

- **Status:** Blocked bởi F-02.
- **Objective:** Typed, fail-fast, secret-safe config cho local/test/prod.
- **Implementation plan:** Define precedence; profile schema; mounted secret/env
  references; validation/redacted Debug; `.env.example`; unsafe dev defaults isolated.
- **Files/modules:** `crates/server/src/config.rs`, `deploy/dev/.env.example`,
  `docs/conventions/config-secrets.md`, config tests.
- **Dependencies/blocks:** F-02; blocks dev stack/server issues.
- **Acceptance criteria:** Missing/invalid config fails startup; no secret in errors;
  prod cannot use dev credentials/profile.
- **Required tests/evidence:** Table/env/file precedence, redaction canary, profile deny.
- **Security/migration:** No committed secrets; rotation contract documented; N/A schema.
- **Out of scope:** Production secret-manager implementation.

## F-08 — Reproducible local development environment

- **Status:** Blocked bởi F-02/F-07.
- **Objective:** One-command CPU-only dev stack, optional GPU profile.
- **Implementation plan:** Pin PG/Qdrant/MinIO/OTel; init buckets/extensions; health/
  seed/reset; named volumes/private network; mock embedding; optional vLLM profile.
- **Files/modules:** `deploy/dev/compose.yml`, init/health/seed/reset scripts,
  `docs/runbooks/local-development.md`.
- **Dependencies/blocks:** F-02/07; blocks Phase 0 spike and server development.
- **Acceptance criteria:** Clean machine up/health/seed/reset/down không console action;
  restart preserves intended data; reset only dev resources.
- **Required tests/evidence:** CI compose smoke, service versions, cold setup transcript.
- **Security/migration:** Non-production credentials/private binds/no secret Git.
- **Out of scope:** Benchmark evidence và production orchestration.

## F-09 — Root task runner, quality tools và CI baseline

- **Status:** Blocked bởi F-03…F-08 và F-10.
- **Objective:** Cùng command local/CI cho format/lint/test/build/dev/migrate.
- **Implementation plan:** Add `just`/equivalent root tasks theo test conventions
  F-10; Rust/TS/SQL checks; dependency/license/security baseline cho cả `app/` và
  `web/`; changed-path optimization nhưng giữ full required gate; pin/bootstrap host
  tools và native Rust/Tauri prerequisites.
- **Files/modules:** `Justfile` hoặc task runner, CI workflows, tool configs,
  `docs/conventions/ci.md`.
- **Dependencies/blocks:** F-03…08 + F-10; blocks all implementation PRs.
- **Acceptance criteria:** Documented commands identical local/CI; failures actionable;
  desktop existing CI vẫn chạy.
- **Required tests/evidence:** Clean checkout full task, cache miss/hit, intentional
  format/lint/test failure fixtures.
- **Security/migration:** Least-privilege CI, pinned actions/tools, no secret artifact.
- **Out of scope:** Production release workflow.

## F-10 — Test pyramid, fixtures và golden-data conventions

- **Status:** Blocked bởi F-03…F-08.
- **Objective:** Chuẩn test/evidence dùng chung trước Phase 0/1A.
- **Implementation plan:** Define unit/integration/contract/denial/E2E/benchmark/
  migration/restore layers; fixture IDs/time/checksum/license; CI artifact retention.
- **Files/modules:** `docs/conventions/testing-fixtures.md`, `tests/fixtures/README.md`,
  sample fixture validators.
- **Dependencies/blocks:** F-03…08; blocks F-09, P0-02 và P1A-01.
- **Acceptance criteria:** Mỗi layer có owner/location/command; fixture synthetic/
  deterministic; large artifacts policy rõ.
- **Required tests/evidence:** Fixture validator catches checksum, absolute path,
  secret canary, duplicate ID.
- **Security/migration:** De-identification/license required; migration evidence format.
- **Out of scope:** Viết toàn bộ golden corpus.

## F-11 — Observability/audit conventions

- **Status:** Blocked bởi F-01/F-06/F-07/F-09.
- **Objective:** Correlation/metrics/log/audit schema ổn định trước business services.
- **Implementation plan:** Field names; request/job/version/signature propagation;
  metric units/cardinality; log allowlist/redaction; audit envelope; sample middleware.
- **Files/modules:** `docs/conventions/observability-audit.md`,
  `crates/server/src/telemetry/`, sample tests/config.
- **Dependencies/blocks:** F-01/06/07/09; blocks 1B telemetry/business routes.
- **Acceptance criteria:** Synthetic in-memory request→job fixture chứng minh field
  propagation/redaction; không thêm durable queue, business route hoặc persisted
  audit trong Phase F; metric naming valid; seeded content/token/key absent.
- **Required tests/evidence:** Trace propagation, cardinality lint, redaction canaries,
  audit fixture.
- **Security/migration:** No document/prompt/token/key/URL/PII; audit schema versioned.
- **Out of scope:** Production dashboards/SIEM.

## F-12 — Contributor workflow, setup docs và foundation gate

- **Status:** Blocked bởi F-01…F-11.
- **Objective:** Chứng minh contributor mới có thể setup và tuân conventions.
- **Implementation plan:** ADR/RFC templates/index; ownership/CODEOWNERS; issue/PR
  templates; Definition of Ready/Done; security triggers; setup/troubleshooting.
- **Files/modules:** `docs/adr/{README,TEMPLATE}.md`, `.github/CODEOWNERS`,
  `.github/{ISSUE_TEMPLATE,PULL_REQUEST_TEMPLATE}.md`, contributor/runbook docs.
- **Dependencies/blocks:** F-01…F-11; blocks Phase 0/1A activation.
- **Acceptance criteria:** Clean-checkout onboarding không tribal knowledge; ownership/
  approval rõ; Phase 0/1A không cần tạo convention riêng.
- **Required tests/evidence:** Independent setup dry run gồm pinned Node/pnpm/Rust/
  task-runner/Compose/native prerequisites; full local/CI task cho app+web; dev stack;
  sample contract/config/telemetry/fixture checks.
- **Security/migration:** Security review triggers và secret incident contact documented.
- **Out of scope:** Benchmark/business implementation/production runbooks.

## Exit gate

Phase F chỉ đóng khi F-12 có clean-checkout evidence, skeleton build, dev stack health,
local/CI quality parity, dependency checks, convention fixtures và contributor setup
độc lập. Sau đó Phase 0 và 1A mới chuyển issue đầu tiên sang `Ready`.
