# Phase F — Engineering foundation và developer platform

## Outcome

Dựng khung kỹ thuật thống nhất trước khi benchmark/refactor/server implementation:

- architecture boundaries và folder skeleton;
- coding/API/data conventions;
- reproducible local development environment;
- formatter/linter/CI/test baseline;
- configuration/secrets/observability conventions;
- contributor documentation và foundation gate.

Phase F không quyết định model, Qdrant topology, SLA hoặc production HA. Các quyết
định cần benchmark vẫn thuộc Phase 0; production deployment vẫn thuộc Phase 4.

## F.1 — Architecture boundaries

Chốt dependency direction:

```text
fileconv-core
    ↑
fileconv-knowledge
    ↑                ↑
desktop adapters    server services
                         ↑
                    axum routes/workers

web → generated API client → server
```

Rules:

- core không biết Tauri/axum/storage;
- knowledge pure mặc định, desktop SQLite/HNSW sau optional features;
- routes không truy cập DB/storage trực tiếp, phải qua service;
- repository business method bắt buộc tenant context;
- web không import Tauri;
- `vendor/markitdown-rs` chỉ tham khảo, không dependency.

## F.2 — Workspace và folder skeleton

Tạo skeleton compile được nhưng không thêm business implementation:

```text
crates/
├── core/
├── knowledge/
└── server/
web/
deploy/
├── dev/
└── scripts/
docs/
├── adr/
├── conventions/
└── runbooks/
bench/markhand_web/
```

Mỗi boundary có README/module ownership. Không tạo generic abstraction khi mới có
một consumer.

## F.3 — Coding conventions

### Rust

- `rustfmt`, clippy policy, error/context rules;
- async/blocking boundary và cancellation;
- no panic/unwrap trong request/worker path;
- DTO/domain/repository/service naming;
- public API docs và unsafe policy.

### TypeScript/React

- strict TypeScript;
- generated API types không sửa tay;
- component/hook/state ownership;
- accessibility, error/loading/empty states;
- browser/Tauri boundary.

### SQL

- naming, UUID/time/enum/check/FK/index;
- tenant column/predicate;
- immutable migrations;
- expand/backfill/cutover/contract;
- transaction/idempotency/locking conventions.

## F.4 — API/event/error conventions

Chốt:

- `/api/v1`, resource naming, pagination/filter/idempotency;
- error `{code,message,requestId,details?}`;
- OpenAPI là contract authority;
- SSE envelope/version/sequence/reconnect;
- date/time/UUID/enum/nullable conventions;
- backward compatibility và deprecation policy.

## F.5 — Configuration và secrets

- typed config, precedence và environment profiles;
- `.env.example` không secret;
- mounted secret/env references;
- redact `Debug`/logs;
- startup fail-fast;
- dev/test/prod không dùng unsafe default lẫn nhau.

## F.6 — Local development environment

Dev stack reproducible cho PostgreSQL, Qdrant, MinIO và telemetry. vLLM/GPU là
optional profile; fake/mock embedding dùng cho CPU-only development.

Yêu cầu:

- one-command up/down/reset/health/seed;
- pinned versions;
- named volumes;
- non-production credentials;
- service ports/private network rõ ràng;
- setup không phụ thuộc thao tác console.

Spike benchmark Phase 0 dùng config riêng, không dùng dev data làm evidence.

## F.7 — Quality tooling và CI

- root task runner cho format/lint/test/build/dev;
- rustfmt/clippy;
- ESLint/Prettier/TypeScript/Vitest;
- SQL migration lint/test;
- dependency/license/security checks baseline;
- cache và changed-path jobs nhưng vẫn có full required gate;
- branch protection checklist.

## F.8 — Test conventions

Định nghĩa test pyramid:

- pure unit;
- repository/adapter integration;
- API contract;
- multi-tenant denial;
- E2E;
- benchmark/golden;
- migration/upgrade/rollback;
- restore/chaos.

Fixture rules:

- synthetic/de-identified;
- deterministic IDs/time;
- checksum/version;
- no secret/absolute machine path;
- large raw results lưu CI artifact, không commit bừa.

## F.9 — Observability/audit conventions

Trước khi thêm service:

- trace/correlation field names;
- metric naming/unit/cardinality;
- structured log allowlist/redaction;
- audit event envelope;
- request/job/document-version/index signature propagation;
- tuyệt đối không log document, prompt, token, key, signed URL hoặc PII.

## F.10 — ADR/RFC và ownership workflow

- ADR template và index;
- khi nào cần ADR, ai approve, supersede thế nào;
- CODEOWNERS/module ownership;
- issue/PR templates;
- Definition of Ready/Done;
- security review trigger.

## Deliverables

- Compileable workspace skeleton.
- Convention documents và automated checks.
- Reproducible CPU-only local dev stack.
- CI foundation và task runner.
- Test/fixture/observability standards.
- ADR/issue/PR workflow.
- Developer setup/runbook.

## Gate

Phase F chỉ pass khi:

- clean checkout setup chạy theo docs;
- workspace skeleton build/test;
- local services up/health/seed/reset;
- formatter/linter/typecheck/test/migration checks chạy cả local và CI;
- dependency boundary tests pass;
- sample API error/SSE/config/telemetry fixtures tuân conventions;
- không secret/PII trong repository hoặc CI artifact mẫu;
- Phase 0 và 1A có thể bắt đầu mà không tự tạo convention riêng.

## Không thuộc phase

- Benchmark/model/topology/SLA decisions.
- Business schema đầy đủ.
- Auth/upload/job/retrieval implementation.
- Production Kubernetes/HA/DR.
- Refactor toàn bộ desktop vào skeleton.
