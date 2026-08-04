<!-- generated-done-issue-plan: 1C-10 -->
# 1C-10 — Rate limit và per-org fairness

Date: 2026-08-04
Base commit: UNKNOWN — not recorded in the source catalog
Source issue: [#112](https://github.com/anhnth24/project-example/issues/112)
Catalog: [`backlog/phase-1c/issues/README.md`](../markhand-web/backlog/phase-1c/issues/README.md)
Phase plan: [`phase-1c-multi-org-security.md`](../markhand-web/phase-1c-multi-org-security.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Rate limit và per-org fairness**.

## Context

- Phase: `1C`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> (run [30678318560](https://github.com/anhnth24/project-example/actions/runs/30678318560)):
> `rust`
> [91310110938](https://github.com/anhnth24/project-example/actions/runs/30678318560/job/91310110938),
> `rust-integration`
> [91310110925](https://github.com/anhnth24/project-example/actions/runs/30678318560/job/91310110925)
> (not path-filter skip / soft pass). Integration log executed
> `tests/noisy_neighbor.rs` (**3 passed; 0 failed**) —
> `noisy_org_backlog_does_not_starve_quiet_org`,
> `slot_exhausted_noisy_org_falls_through_to_quiet_org_same_cycle`, plus shared
> worker-context pin. Fast unit coverage for per-org rate bucket and
> `workers::fairness::OrgRotation` green in `rust` job. Fairness SLO wall-clock
> remains **1C-13**; GPU semaphore remains N/A-until-GPU.

## Implementation plan

User/IP/auth limits (**done từ trước**); per-org API bucket (**done**:
`middleware/rate_limit.rs::check_org` + `routes/rate_limit_guard.rs`); per-org worker
fairness (**done**: `workers/fairness.rs::OrgRotation` + `bin/worker.rs` multi-org
env); GPU scheduler/semaphore (**N/A-until-GPU**, xem điều kiện kích hoạt ở trên);
headers, privacy-safe metrics (**done từ trước**, scope mới `org` trong 429 details
là giá trị tĩnh, không PII).

## Files/modules

The source catalog records implementation and file scope together; see **Implementation plan** above.

## Dependencies / blocks

1C-09 (dùng lại reservation `concurrent_jobs`) + Phase 0 SLO/capacity

## Acceptance criteria

Noisy org không bỏ đói org
khác — **done ở mức deterministic-count** (`tests/noisy_neighbor.rs` DB-gated + unit
fairness/rate-limit fast); burst/window/crash-release/proxy tests đã có từ trước
(`rate_limit.rs` tests, `sse_stream_readiness.rs` trusted-proxy 429); riêng
**fairness SLO wall-clock** chuyển 1C-13.

## Required tests / evidence

Noisy org không bỏ đói org
khác — **done ở mức deterministic-count** (`tests/noisy_neighbor.rs` DB-gated + unit
fairness/rate-limit fast); burst/window/crash-release/proxy tests đã có từ trước
(`rate_limit.rs` tests, `sse_stream_readiness.rs` trusted-proxy 429); riêng
**fairness SLO wall-clock** chuyển 1C-13.

## Security and migration notes

Chỉ trusted proxy IP, bounded state (org bucket nằm trong
cùng HashMap HARD_CAP 10k key, evictable như các key khác). Không migration mới.

## Out of scope

multi-region; per-org tiered rate limit từ `org_quotas` (đợi yêu cầu tier
thật); GPU semaphore (until-GPU).

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Completion/evidence commits

- `30678318560`
- `6833f57d94949c75ea36609e1055a1139e097c8a`
- `91310110925`
- `91310110938`

- GitHub sync-closed timestamp: `2026-08-01T04:06:37Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
