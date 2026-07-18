#!/usr/bin/env bash
set -euo pipefail
REPO='anhnth24/project-example'

ensure_milestone() {
  local title="$1"
  local description="$2"
  local number
  number=$(gh api "repos/${REPO}/milestones?state=all&per_page=100" \
    --jq ".[] | select(.title==\"$title\") | .number" | head -n1 || true)
  if [ -z "${number:-}" ]; then
    gh api --method POST "repos/${REPO}/milestones" \
      -f title="$title" \
      -f description="$description" \
      -f state=open >/dev/null
    echo "milestone created: $title"
    return 0
  fi
  gh api --method PATCH "repos/${REPO}/milestones/${number}" \
    -f state=open \
    -f description="$description" >/dev/null
  echo "milestone updated: $title (#${number})"
}

create_if_missing() {
  local title="$1"
  if gh issue list --repo "$REPO" --state all --search "in:title \"$title\"" --json number --jq 'length' | grep -qv '^0$'; then
    echo "skip existing issue: $title"
    return 0
  fi
  shift
  gh issue create --repo "$REPO" "$@"
  echo "issue created: $title"
}

echo "Ensuring Markhand Web milestones..."

ensure_milestone 'Phase F — Engineering foundation' 'Markhand Web phase `F`.

**Outcome:** Engineering rules, skeleton, local dev environment và CI foundation
**Issues:** 12
**Phase plan:** `phase-f-engineering-foundation.md`
**Issue catalog:** `backlog/phase-f/issues/README.md`'

ensure_milestone 'Phase 0 — Discovery & Gates' 'Markhand Web phase `0`.

**Outcome:** Chốt bằng số liệu: scale, retrieval, bảo mật upload, SLA/RPO/RTO
**Issues:** 10
**Phase plan:** `phase-0-discovery-and-gates.md`
**Issue catalog:** `backlog/phase-0/issues/README.md`'

ensure_milestone 'Phase 1A — Knowledge Extraction' 'Markhand Web phase `1A`.

**Outcome:** Tách logic RAG dùng chung thành crates/knowledge, desktop không đổi hành vi
**Issues:** 10
**Phase plan:** `phase-1a-knowledge-extraction.md`
**Issue catalog:** `backlog/phase-1a/issues/README.md`'

ensure_milestone 'Phase 1B — Single-org POC' 'Markhand Web phase `1B`.

**Outcome:** POC single-org hoàn chỉnh: upload → convert → index → Q&A citation
**Issues:** 24
**Phase plan:** `phase-1b-single-org-poc.md`
**Issue catalog:** `backlog/phase-1b/issues/README.md`'

ensure_milestone 'Phase 1C — Multi-org Security' 'Markhand Web phase `1C`.

**Outcome:** Multi-org, RBAC/ACL, quota atomic và denial test
**Issues:** 13
**Phase plan:** `phase-1c-multi-org-security.md`
**Issue catalog:** `backlog/phase-1c/issues/README.md`'

ensure_milestone 'Phase 2 — Web SPA MVP' 'Markhand Web phase `2`.

**Outcome:** Web SPA MVP: login, library, Q&A, admin tối thiểu
**Issues:** 16
**Phase plan:** `phase-2-web-spa.md`
**Issue catalog:** `backlog/phase-2/issues/README.md`'

ensure_milestone 'Phase 3 — Document Intelligence' 'Markhand Web phase `3`.

**Outcome:** Port intelligence: BRD/PRD, quality, PII, bảng, version, export
**Issues:** 14
**Phase plan:** `phase-3-intelligence.md`
**Issue catalog:** `backlog/phase-3/issues/README.md`'

ensure_milestone 'Phase 4 — Production Hardening' 'Markhand Web phase `4`.

**Outcome:** OIDC/SSO, hardening production, DR và onboarding/help
**Issues:** 14
**Phase plan:** `phase-4-production-hardening.md`
**Issue catalog:** `backlog/phase-4/issues/README.md`'

echo "Ensuring Markhand Web issues..."

create_if_missing "F-01 \u2014 Architecture boundaries v\u00e0 dependency rules" \
  --title "F-01 \u2014 Architecture boundaries v\u00e0 dependency rules" \
  --body "## Metadata\n\n- Milestone: Phase F \u2014 Engineering foundation\n- Phase code: F\n- Issue ID: F-01\n- Status: `done`\n- Catalog: `backlog/phase-f/issues/README.md`\n- Phase plan: `phase-f-engineering-foundation.md`\n\n## Objective\n\nKh\u00f3a dependency direction v\u00e0 module responsibilities tr\u01b0\u1edbc scaffold.\n\n## Implementation plan\n\nVi\u1ebft architecture boundary ADR; define allowed/forbidden\ndependencies; route\u2192service\u2192repository; tenant context rule; browser/Tauri split;\nautomated `cargo tree`/import checks. Bootstrap minimum CODEOWNERS, issue/PR\ntemplate, Definition of Ready/Done v\u00e0 security-review triggers \u0111\u1ec3 govern ch\u00ednh\nPhase F; F-12 ho\u00e0n thi\u1ec7n v\u00e0 ki\u1ec3m ch\u1ee9ng workflow.\n\n## Files/modules\n\n`docs/adr/0001-web-boundaries.md` (new),\n`docs/conventions/dependencies.md` (new), `.github/CODEOWNERS`,\n`.github/{ISSUE_TEMPLATE,PULL_REQUEST_TEMPLATE}.md`, CI boundary scripts.\n\n## Dependencies / blocks\n\nKh\u00f4ng; blocks F-02 v\u00e0 m\u1ecdi crate/web implementation.\n\n## Acceptance criteria\n\nCore kh\u00f4ng framework/storage; knowledge pure m\u1eb7c \u0111\u1ecbnh;\nserver kh\u00f4ng reverse-depend desktop; web kh\u00f4ng Tauri; vendor kh\u00f4ng dependency.\n\n## Required tests / evidence\n\nPositive/negative sample boundary checks trong CI;\narchitecture diagram v\u00e0 approver.\n\n## Security and migration notes\n\nTenant context b\u1eaft bu\u1ed9c \u1edf repository rule; migration N/A.\n\n## Out of scope\n\nStorage trait t\u1ed5ng qu\u00e1t v\u00e0 business implementation.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase F \u2014 Engineering foundation" \
  --label "markhand-web,docs,web-foundation"

create_if_missing "F-02 \u2014 Workspace v\u00e0 folder skeleton" \
  --title "F-02 \u2014 Workspace v\u00e0 folder skeleton" \
  --body "## Metadata\n\n- Milestone: Phase F \u2014 Engineering foundation\n- Phase code: F\n- Issue ID: F-02\n- Status: `done`\n- Catalog: `backlog/phase-f/issues/README.md`\n- Phase plan: `phase-f-engineering-foundation.md`\n\n## Objective\n\nT\u1ea1o khung compile \u0111\u01b0\u1ee3c cho knowledge/server/web/deploy/docs/bench.\n\n## Implementation plan\n\nAdd workspace members v\u1edbi minimal libraries/binaries; module\nREADMEs/ownership; Vite web shell; deploy/dev placeholders; kh\u00f4ng copy business\nlogic. Ch\u1ed1t root pnpm workspace/lockfile policy cho `app/` + `web/`; pin Node,\npnpm, task runner v\u00e0 Compose requirements; th\u00eam bootstrap/version-check command.\n\n## Files/modules\n\n`Cargo.toml`, `crates/{knowledge,server}/`, `web/`, `deploy/dev/`,\n`docs/{adr,conventions,runbooks}/`, `bench/markhand_web/`.\n\n## Dependencies / blocks\n\nF-01; blocks coding/tooling/dev environment issues.\n\n## Acceptance criteria\n\nCargo workspace v\u00e0 web build; server API/worker binaries start\nhelp/config validation only; no cyclic/forbidden deps; JS workspace/lockfile policy\nv\u00e0 host tool versions \u0111\u01b0\u1ee3c m\u00e1y ki\u1ec3m tra.\n\n## Required tests / evidence\n\n`cargo metadata/check`, bootstrap/version check,\n`pnpm install --frozen-lockfile`, app+web build, tree/import boundary.\n\n## Security and migration notes\n\nNo credential/default public bind; no DB migration.\n\n## Out of scope\n\nAuth/schema/routes/jobs.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase F \u2014 Engineering foundation" \
  --label "markhand-web,docs,web-foundation"

create_if_missing "F-03 \u2014 Rust coding v\u00e0 crate conventions" \
  --title "F-03 \u2014 Rust coding v\u00e0 crate conventions" \
  --body "## Metadata\n\n- Milestone: Phase F \u2014 Engineering foundation\n- Phase code: F\n- Issue ID: F-03\n- Status: `done`\n- Catalog: `backlog/phase-f/issues/README.md`\n- Phase plan: `phase-f-engineering-foundation.md`\n\n## Objective\n\nM\u1ed9t chu\u1ea9n Rust b\u1eaft bu\u1ed9c cho core/knowledge/server/workers.\n\n## Implementation plan\n\nRustfmt/clippy policy; error/context; async vs blocking;\ncancellation/timeouts; panic/unwrap/unsafe/public docs; naming/module visibility.\n\n## Files/modules\n\n`rustfmt.toml`, `clippy.toml` n\u1ebfu c\u1ea7n,\n`docs/conventions/rust.md`, root lint task, CI.\n\n## Dependencies / blocks\n\nF-02; blocks Rust feature issues.\n\n## Acceptance criteria\n\nConvention c\u00f3 enforceable rule + justified exceptions;\nexisting code c\u00f3 migration plan thay v\u00ec b\u1eadt deny ph\u00e1 to\u00e0n repo ngay.\n\n## Required tests / evidence\n\nFormat check, clippy selected warnings-as-errors,\nforbidden-pattern baseline/delta.\n\n## Security and migration notes\n\nRequest/worker path kh\u00f4ng panic; secret-safe errors; N/A schema.\n\n## Out of scope\n\nRefactor to\u00e0n b\u1ed9 warning c\u0169 trong c\u00f9ng issue.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase F \u2014 Engineering foundation" \
  --label "markhand-web,docs,web-foundation"

create_if_missing "F-04 \u2014 TypeScript/React conventions" \
  --title "F-04 \u2014 TypeScript/React conventions" \
  --body "## Metadata\n\n- Milestone: Phase F \u2014 Engineering foundation\n- Phase code: F\n- Issue ID: F-04\n- Status: `done`\n- Catalog: `backlog/phase-f/issues/README.md`\n- Phase plan: `phase-f-engineering-foundation.md`\n\n## Objective\n\nChu\u1ea9n strict TS, component/hook/state v\u00e0 accessibility cho web.\n\n## Implementation plan\n\nTS strict policy; generated API immutable; naming/import\nboundaries; state ownership; loading/error/empty; abort cleanup; a11y checklist.\n\n## Files/modules\n\n`web/tsconfig*.json`, ESLint/Prettier config,\n`docs/conventions/typescript-react.md`, web test setup.\n\n## Dependencies / blocks\n\nF-02; blocks Phase 2 implementation.\n\n## Acceptance criteria\n\nNo Tauri imports; generated code separated; hooks clean up\nrequests/streams; component patterns documented.\n\n## Required tests / evidence\n\nTypecheck/lint/format/unit sample/a11y smoke.\n\n## Security and migration notes\n\nNo token/content logging or unsafe HTML by default; N/A schema.\n\n## Out of scope\n\nFull design system v\u00e0 Phase 2 pages.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase F \u2014 Engineering foundation" \
  --label "markhand-web,docs,web-foundation"

create_if_missing "F-05 \u2014 SQL/data/migration conventions" \
  --title "F-05 \u2014 SQL/data/migration conventions" \
  --body "## Metadata\n\n- Milestone: Phase F \u2014 Engineering foundation\n- Phase code: F\n- Issue ID: F-05\n- Status: `done`\n- Catalog: `backlog/phase-f/issues/README.md`\n- Phase plan: `phase-f-engineering-foundation.md`\n\n## Objective\n\nNg\u0103n schema/tenant/migration conventions b\u1ecb ph\u00e1t minh theo t\u1eebng PR.\n\n## Implementation plan\n\nNaming/types/time/UUID/FK/check/index; `org_id`; transaction/\nlocking/idempotency; immutable migration; expand/backfill/cutover/contract; rollback.\n\n## Files/modules\n\n`docs/conventions/sql-migrations.md`,\n`crates/server/migrations/README.md`, migration test harness skeleton.\n\n## Dependencies / blocks\n\nF-01/02; blocks Phase 1B schema.\n\n## Acceptance criteria\n\nExample migration/repository query h\u1ee3p conventions; policy\nfresh/upgrade/mixed-version r\u00f5.\n\n## Required tests / evidence\n\nEmpty DB apply, migration checksum/immutability,\nrollback-compat sample.\n\n## Security and migration notes\n\nTenant predicate/RLS review checklist b\u1eaft bu\u1ed9c.\n\n## Out of scope\n\nBusiness tables v\u00e0 RLS decision.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase F \u2014 Engineering foundation" \
  --label "markhand-web,docs,web-foundation"

create_if_missing "F-06 \u2014 REST/OpenAPI/SSE/error conventions" \
  --title "F-06 \u2014 REST/OpenAPI/SSE/error conventions" \
  --body "## Metadata\n\n- Milestone: Phase F \u2014 Engineering foundation\n- Phase code: F\n- Issue ID: F-06\n- Status: `done`\n- Catalog: `backlog/phase-f/issues/README.md`\n- Phase plan: `phase-f-engineering-foundation.md`\n\n## Objective\n\nContract th\u1ed1ng nh\u1ea5t \u0111\u1ec3 backend/web kh\u00f4ng drift.\n\n## Implementation plan\n\n`/api/v1`; resources/pagination/idempotency; canonical error;\ndate/UUID/enum/null; OpenAPI authority; SSE envelope/version/sequence/reconnect;\ndeprecation policy.\n\n## Files/modules\n\n`docs/conventions/api.md`, `crates/server/openapi/`,\nsample DTO/error/SSE types v\u00e0 fixtures.\n\n## Dependencies / blocks\n\nF-01/02; blocks 1B routes v\u00e0 Phase 2 client.\n\n## Acceptance criteria\n\nSample contract generate TS; error/SSE fixtures round-trip;\ncompatibility rules c\u00f3 examples.\n\n## Required tests / evidence\n\nOpenAPI validation/snapshot, Rust\u2194TS fixture,\nSSE parser sequence sample.\n\n## Security and migration notes\n\nErrors kh\u00f4ng leak internal; SSE auth/revocation requirements;\npersisted migration N/A.\n\n## Out of scope\n\nBusiness endpoints.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase F \u2014 Engineering foundation" \
  --label "markhand-web,docs,web-foundation"

create_if_missing "F-07 \u2014 Configuration, secrets v\u00e0 environment profiles" \
  --title "F-07 \u2014 Configuration, secrets v\u00e0 environment profiles" \
  --body "## Metadata\n\n- Milestone: Phase F \u2014 Engineering foundation\n- Phase code: F\n- Issue ID: F-07\n- Status: `done`\n- Catalog: `backlog/phase-f/issues/README.md`\n- Phase plan: `phase-f-engineering-foundation.md`\n\n## Objective\n\nTyped, fail-fast, secret-safe config cho local/test/prod.\n\n## Implementation plan\n\nDefine precedence; profile schema; mounted secret/env\nreferences; validation/redacted Debug; `.env.example`; unsafe dev defaults isolated.\n\n## Files/modules\n\n`crates/server/src/config.rs`, `deploy/dev/.env.example`,\n`docs/conventions/config-secrets.md`, config tests.\n\n## Dependencies / blocks\n\nF-02; blocks dev stack/server issues.\n\n## Acceptance criteria\n\nMissing/invalid config fails startup; no secret in errors;\nprod cannot use dev credentials/profile.\n\n## Required tests / evidence\n\nTable/env/file precedence, redaction canary, profile deny.\n\n## Security and migration notes\n\nNo committed secrets; rotation contract documented; N/A schema.\n\n## Out of scope\n\nProduction secret-manager implementation.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase F \u2014 Engineering foundation" \
  --label "markhand-web,docs,web-foundation"

create_if_missing "F-08 \u2014 Reproducible local development environment" \
  --title "F-08 \u2014 Reproducible local development environment" \
  --body "## Metadata\n\n- Milestone: Phase F \u2014 Engineering foundation\n- Phase code: F\n- Issue ID: F-08\n- Status: `done`\n- Catalog: `backlog/phase-f/issues/README.md`\n- Phase plan: `phase-f-engineering-foundation.md`\n\n## Objective\n\nOne-command CPU-only dev stack, optional GPU profile.\n\n## Implementation plan\n\nPin PG/Qdrant/MinIO/OTel; init buckets/extensions; health/\nseed/reset; named volumes/private network; mock embedding; optional vLLM profile.\n\n## Files/modules\n\n`deploy/dev/compose.yml`, init/health/seed/reset scripts,\n`docs/runbooks/local-development.md`.\n\n## Dependencies / blocks\n\nF-02/07; blocks Phase 0 spike and server development.\n\n## Acceptance criteria\n\nClean machine up/health/seed/reset/down kh\u00f4ng console action;\nrestart preserves intended data; reset only dev resources.\n\n## Required tests / evidence\n\nCI compose smoke, service versions, cold setup transcript.\n\n## Security and migration notes\n\nNon-production credentials/private binds/no secret Git.\n\n## Out of scope\n\nBenchmark evidence v\u00e0 production orchestration.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase F \u2014 Engineering foundation" \
  --label "markhand-web,docs,web-foundation"

create_if_missing "F-09 \u2014 Root task runner, quality tools v\u00e0 CI baseline" \
  --title "F-09 \u2014 Root task runner, quality tools v\u00e0 CI baseline" \
  --body "## Metadata\n\n- Milestone: Phase F \u2014 Engineering foundation\n- Phase code: F\n- Issue ID: F-09\n- Status: `done`\n- Catalog: `backlog/phase-f/issues/README.md`\n- Phase plan: `phase-f-engineering-foundation.md`\n\n## Objective\n\nC\u00f9ng command local/CI cho format/lint/test/build/dev/migrate.\n\n## Implementation plan\n\nAdd `just`/equivalent root tasks theo test conventions\nF-10; Rust/TS/SQL checks; dependency/license/security baseline cho c\u1ea3 `app/` v\u00e0\n`web/`; changed-path optimization nh\u01b0ng gi\u1eef full required gate; pin/bootstrap host\ntools v\u00e0 native Rust/Tauri prerequisites.\n\n## Files/modules\n\n`Justfile` ho\u1eb7c task runner, CI workflows, tool configs,\n`docs/conventions/ci.md`.\n\n## Dependencies / blocks\n\nF-03\u202608 + F-10; blocks all implementation PRs.\n\n## Acceptance criteria\n\nDocumented commands identical local/CI; failures actionable;\ndesktop existing CI v\u1eabn ch\u1ea1y.\n\n## Required tests / evidence\n\nClean checkout full task, cache miss/hit, intentional\nformat/lint/test failure fixtures.\n\n## Security and migration notes\n\nLeast-privilege CI, pinned actions/tools, no secret artifact.\n\n## Out of scope\n\nProduction release workflow.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase F \u2014 Engineering foundation" \
  --label "markhand-web,docs,web-foundation"

create_if_missing "F-10 \u2014 Test pyramid, fixtures v\u00e0 golden-data conventions" \
  --title "F-10 \u2014 Test pyramid, fixtures v\u00e0 golden-data conventions" \
  --body "## Metadata\n\n- Milestone: Phase F \u2014 Engineering foundation\n- Phase code: F\n- Issue ID: F-10\n- Status: `done`\n- Catalog: `backlog/phase-f/issues/README.md`\n- Phase plan: `phase-f-engineering-foundation.md`\n\n## Objective\n\nChu\u1ea9n test/evidence d\u00f9ng chung tr\u01b0\u1edbc Phase 0/1A.\n\n## Implementation plan\n\nDefine unit/integration/contract/denial/E2E/benchmark/\nmigration/restore layers; fixture IDs/time/checksum/license; CI artifact retention.\n\n## Files/modules\n\n`docs/conventions/testing-fixtures.md`, `tests/fixtures/README.md`,\nsample fixture validators.\n\n## Dependencies / blocks\n\nF-03\u202608; blocks F-09, P0-02 v\u00e0 P1A-01.\n\n## Acceptance criteria\n\nM\u1ed7i layer c\u00f3 owner/location/command; fixture synthetic/\ndeterministic; large artifacts policy r\u00f5.\n\n## Required tests / evidence\n\nFixture validator catches checksum, absolute path,\nsecret canary, duplicate ID.\n\n## Security and migration notes\n\nDe-identification/license required; migration evidence format.\n\n## Out of scope\n\nVi\u1ebft to\u00e0n b\u1ed9 golden corpus.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase F \u2014 Engineering foundation" \
  --label "markhand-web,docs,web-foundation"

create_if_missing "F-11 \u2014 Observability/audit conventions" \
  --title "F-11 \u2014 Observability/audit conventions" \
  --body "## Metadata\n\n- Milestone: Phase F \u2014 Engineering foundation\n- Phase code: F\n- Issue ID: F-11\n- Status: `done`\n- Catalog: `backlog/phase-f/issues/README.md`\n- Phase plan: `phase-f-engineering-foundation.md`\n\n## Objective\n\nCorrelation/metrics/log/audit schema \u1ed5n \u0111\u1ecbnh tr\u01b0\u1edbc business services.\n\n## Implementation plan\n\nField names; request/job/version/signature propagation;\nmetric units/cardinality; log allowlist/redaction; audit envelope; sample middleware.\n\n## Files/modules\n\n`docs/conventions/observability-audit.md`,\n`crates/server/src/telemetry/`, sample tests/config.\n\n## Dependencies / blocks\n\nF-01/06/07/09; blocks 1B telemetry/business routes.\n\n## Acceptance criteria\n\nSynthetic in-memory request\u2192job fixture ch\u1ee9ng minh field\npropagation/redaction; kh\u00f4ng th\u00eam durable queue, business route ho\u1eb7c persisted\naudit trong Phase F; metric naming valid; seeded content/token/key absent.\n\n## Required tests / evidence\n\nTrace propagation, cardinality lint, redaction canaries,\naudit fixture.\n\n## Security and migration notes\n\nNo document/prompt/token/key/URL/PII; audit schema versioned.\n\n## Out of scope\n\nProduction dashboards/SIEM.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase F \u2014 Engineering foundation" \
  --label "markhand-web,docs,web-foundation"

create_if_missing "F-12 \u2014 Contributor workflow, setup docs v\u00e0 foundation gate" \
  --title "F-12 \u2014 Contributor workflow, setup docs v\u00e0 foundation gate" \
  --body "## Metadata\n\n- Milestone: Phase F \u2014 Engineering foundation\n- Phase code: F\n- Issue ID: F-12\n- Status: `done`\n- Catalog: `backlog/phase-f/issues/README.md`\n- Phase plan: `phase-f-engineering-foundation.md`\n\n## Objective\n\nCh\u1ee9ng minh contributor m\u1edbi c\u00f3 th\u1ec3 setup v\u00e0 tu\u00e2n conventions.\n\n## Implementation plan\n\nADR/RFC templates/index; ownership/CODEOWNERS; issue/PR\ntemplates; Definition of Ready/Done; security triggers; setup/troubleshooting.\n\n## Files/modules\n\n`docs/adr/{README,TEMPLATE}.md`, `.github/CODEOWNERS`,\n`.github/{ISSUE_TEMPLATE,PULL_REQUEST_TEMPLATE}.md`, contributor/runbook docs.\n\n## Dependencies / blocks\n\nF-01\u2026F-11; blocks Phase 0/1A activation.\n\n## Acceptance criteria\n\nClean-checkout onboarding kh\u00f4ng tribal knowledge; ownership/\napproval r\u00f5; Phase 0/1A kh\u00f4ng c\u1ea7n t\u1ea1o convention ri\u00eang.\n\n## Required tests / evidence\n\nIndependent setup dry run g\u1ed3m pinned Node/pnpm/Rust/\ntask-runner/Compose/native prerequisites; full local/CI task cho app+web; dev stack;\nsample contract/config/telemetry/fixture checks.\n\n## Security and migration notes\n\nSecurity review triggers v\u00e0 secret incident contact documented.\n\n## Out of scope\n\nBenchmark/business implementation/production runbooks.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase F \u2014 Engineering foundation" \
  --label "markhand-web,docs,web-foundation"

create_if_missing "P0-01 \u2014 Kh\u00f3a workload, hardware v\u00e0 gate registry" \
  --title "P0-01 \u2014 Kh\u00f3a workload, hardware v\u00e0 gate registry" \
  --body "## Metadata\n\n- Milestone: Phase 0 \u2014 Discovery & Gates\n- Phase code: 0\n- Issue ID: P0-01\n- Status: `done`\n- Catalog: `backlog/phase-0/issues/README.md`\n- Phase plan: `phase-0-discovery-and-gates.md`\n\n## Objective\n\nThay gi\u1ea3 \u0111\u1ecbnh scale/SLA b\u1eb1ng workload envelope, hardware profile v\u00e0\ngate schema \u0111\u01b0\u1ee3c duy\u1ec7t.\n\n## Implementation plan\n\nGhi org/collection/document/vector, ingest/query/recovery load; CPU/RAM/\ndisk/GPU/network; t\u1ea1o registry g\u1ed3m metric, workload, threshold, command,\nenvironment, approver v\u00e0 failure disposition.\n\n## Files/modules\n\n`bench/markhand_web/{README.md,workload-profile.yaml,gates.yaml}`,\n`bench/markhand_web/environments/`, `docs/adr/README.md`.\n\n## Dependencies / blocks\n\nC\u1ea7n input s\u1ea3n ph\u1ea9m/v\u1eadn h\u00e0nh; block m\u1ecdi benchmark.\n\n## Acceptance criteria\n\nNormal/peak/recovery/aggregate load \u0111\u1ea7y \u0111\u1ee7; m\u1ecdi open decision c\u00f3\nowner; gate thi\u1ebfu tr\u01b0\u1eddng b\u1ecb schema validator t\u1eeb ch\u1ed1i.\n\n## Required tests / evidence\n\nValidate YAML/schema; m\u1ecdi report sau emit environment\nfingerprint.\n\n## Security and migration notes\n\nKh\u00f4ng ghi credential, hostname n\u1ed9i b\u1ed9 ho\u1eb7c t\u00ean kh\u00e1ch h\u00e0ng.\n\n## Out of scope\n\nCh\u1ecdn model v\u00e0 tuy\u00ean b\u1ed1 \u0111\u1ea1t SLA.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 0 \u2014 Discovery & Gates" \
  --label "markhand-web,docs,web-p0"

create_if_missing "P0-02 \u2014 Golden corpus ti\u1ebfng Vi\u1ec7t v\u00e0 adversarial corpus" \
  --title "P0-02 \u2014 Golden corpus ti\u1ebfng Vi\u1ec7t v\u00e0 adversarial corpus" \
  --body "## Metadata\n\n- Milestone: Phase 0 \u2014 Discovery & Gates\n- Phase code: 0\n- Issue ID: P0-02\n- Status: `done`\n- Catalog: `backlog/phase-0/issues/README.md`\n- Phase plan: `phase-0-discovery-and-gates.md`\n\n## Objective\n\nDataset t\u00e1i l\u1eadp cho conversion, retrieval, citation v\u00e0 upload attack.\n\n## Implementation plan\n\nTh\u00eam m\u1ecdi format; 200\u2013500 query v\u1edbi expected document/source span/\nrelevance/no-answer; multi-document v\u00e0 immutable multi-version citations\n(`current`/`as_of`/`compare`/`history`); BA/design/dev cross-document conflict\nlifecycle (open/resolved/history) v\u1edbi cited claims; sample spoof/bomb/malformed/\ntraversal/prompt injection; pin checksum v\u00e0 provenance/license.\n\n## Files/modules\n\n`bench/markhand_web/golden/`, `adversarial/`,\n`manifest.lock.json`, `scripts/validate_corpus.py`.\n\n## Dependencies / blocks\n\nP0-01; fixture ph\u1ea3i redistributable.\n\n## Acceptance criteria\n\nCoverage \u0111\u1ee7 category; source/version span \u1ed5n \u0111\u1ecbnh; current fact kh\u00f4ng\ntr\u1ecf version c\u0169; compare/history cite \u0111\u1ee7 old+new v\u00e0 delta; conflict current/history\ncite \u0111\u1ee7 hai ph\u00eda + resolution versions; validator b\u1eaft checksum, duplicate ID, invalid\nspan/version/conflict lineage v\u00e0 missing license; m\u1ed7i attack c\u00f3 expected disposition.\n\n## Required tests / evidence\n\nClean-checkout reproducibility; dual review + adjudication.\n\n## Security and migration notes\n\nSynthetic/de-identified; bomb fixture ch\u1ec9 ch\u1ea1y trong limits.\n\n## Out of scope\n\nCustomer data v\u00e0 expected chunk ID tr\u01b0\u1edbc khi ch\u1ed1t chunking.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 0 \u2014 Discovery & Gates" \
  --label "markhand-web,docs,web-p0"

create_if_missing "P0-03 \u2014 M\u1edf r\u1ed9ng desktop baseline tr\u00ean corpus Phase 0" \
  --title "P0-03 \u2014 M\u1edf r\u1ed9ng desktop baseline tr\u00ean corpus Phase 0" \
  --body "## Metadata\n\n- Milestone: Phase 0 \u2014 Discovery & Gates\n- Phase code: 0\n- Issue ID: P0-03\n- Status: `done`\n- Catalog: `backlog/phase-0/issues/README.md`\n- Phase plan: `phase-0-discovery-and-gates.md`\n\n## Objective\n\nM\u1edf r\u1ed9ng parity baseline P1A-01 l\u00ean corpus/metrics Phase 0; P1A-01 l\u00e0\nbaseline authoritative \u0111\u1ec3 vi\u1ec7c extraction kh\u00f4ng ph\u1ea3i \u0111\u1ee3i to\u00e0n b\u1ed9 corpus.\n\n## Implementation plan\n\nT\u00e1i d\u00f9ng fixtures/harness P1A-01; ch\u1ea1y release conversion; snapshot top-k,\nscores, anchors, answer modes,\nwarnings, stats, provider fallback v\u00e0 signature mismatch.\n\n## Files/modules\n\n`bench/markhand_web/scripts/run_desktop_baseline.sh`,\n`baselines/desktop-v1/`, `reports/desktop-baseline.md`.\n\n## Dependencies / blocks\n\nP0-02 + P1A-01 authoritative parity harness; provider run\nc\u1ea7n config/model pin.\n\n## Acceptance criteria\n\nM\u1ecdi format/query c\u00f3 raw machine-readable result; offline ch\u1ea1y kh\u00f4ng\nc\u1ea7n LLM; \u0111\u1ee7 d\u1eef li\u1ec7u so parity 1A.\n\n## Required tests / evidence\n\nCER/WER/time, Recall@5/10, MRR, nDCG, citation correctness;\ndeterministic rerun.\n\n## Security and migration notes\n\nRedact key/prompt/absolute path.\n\n## Out of scope\n\nS\u1eeda defect ranking/performance.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 0 \u2014 Discovery & Gates" \
  --label "markhand-web,docs,web-p0"

create_if_missing "P0-04 \u2014 Spike infrastructure t\u00e1i l\u1eadp" \
  --title "P0-04 \u2014 Spike infrastructure t\u00e1i l\u1eadp" \
  --body "## Metadata\n\n- Milestone: Phase 0 \u2014 Discovery & Gates\n- Phase code: 0\n- Issue ID: P0-04\n- Status: `done`\n- Catalog: `backlog/phase-0/issues/README.md`\n- Phase plan: `phase-0-discovery-and-gates.md`\n\n## Objective\n\nStack disposable PG/Qdrant/MinIO/vLLM/telemetry cho benchmark.\n\n## Implementation plan\n\nT\u00e1i d\u00f9ng compose/services/scripts base t\u1eeb F-08; th\u00eam benchmark-specific\noverride v\u1edbi isolated volumes/data, vLLM/GPU profile, workload sizing, image digest\nv\u00e0 environment fingerprint. Kh\u00f4ng fork dev stack.\n\n## Files/modules\n\n`deploy/compose.spike.yml`, `deploy/spike/`, base `deploy/dev/`,\n`bench/markhand_web/scripts/spike-{health,reset}.sh`.\n\n## Dependencies / blocks\n\nPhase F/F-08 + P0-01; target hardware \u0111\u1ec3 \u0111\u00f3ng issue.\n\n## Acceptance criteria\n\nM\u1ed9t command boot t\u1eeb empty volumes; kh\u00f4ng thao t\u00e1c console; restart/\nreset \u0111\u00fang semantics.\n\n## Required tests / evidence\n\nClean-machine startup, service health, version/GPU/telemetry\nfingerprint.\n\n## Security and migration notes\n\nBind private/localhost; non-production secrets ngo\u00e0i Git.\n\n## Out of scope\n\nHA/TLS/production orchestration.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 0 \u2014 Discovery & Gates" \
  --label "markhand-web,docs,web-p0"

create_if_missing "P0-05 \u2014 \u0110\u00e1nh gi\u00e1 embedding ti\u1ebfng Vi\u1ec7t" \
  --title "P0-05 \u2014 \u0110\u00e1nh gi\u00e1 embedding ti\u1ebfng Vi\u1ec7t" \
  --body "## Metadata\n\n- Milestone: Phase 0 \u2014 Discovery & Gates\n- Phase code: 0\n- Issue ID: P0-05\n- Status: `ready`\n- Catalog: `backlog/phase-0/issues/README.md`\n- Phase plan: `phase-0-discovery-and-gates.md`\n\n## Objective\n\nCh\u1ed1t provider/model/revision/dimension/normalization \u0111\u1ee7 \u0111\u1ec3 l\u1eadp\ntr\u00ecnh Phase 0\u21921B; gi\u1eef \u0111\u01b0\u1eddng c\u1eaft sang on-prem vLLM.\n\n## Implementation plan\n\nInterim: so GLM `embedding-3` (v\u00e0 `embedding-2` n\u1ebfu c\u1ea7n) qua\nOpenAI-compatible API + `FILECONV_EMBEDDING_API_KEY`; pin tokenizer/batch/\ntruncation/dimensions/normalize; \u0111o theo category v\u00e0 API latency. Target (sau):\nso `bge-m3` v\u00e0 multilingual-e5 tr\u00ean Profile B GPU/vLLM (VRAM, saturation).\n\n## Files/modules\n\n`bench/markhand_web/embedding/`, `scripts/run_embedding_eval.py`,\n`reports/embedding-evaluation.md`, `docs/adr/0004-interim-glm-cloud-embedding.md`.\n\n## Dependencies / blocks\n\nCorpus + spike + GLM credential; kh\u00f4ng c\u00f2n b\u1eaft bu\u1ed9c\ntarget GPU \u0111\u1ec3 \u0111\u00f3ng interim. Cutover vLLM v\u1eabn c\u1ea7n Profile B + approved model download.\n\n## Required tests / evidence\n\nInterim: Recall/MRR/nDCG, API P50/P95/P99, vectors/s \u01b0\u1edbc l\u01b0\u1ee3ng,\nfailure rate; \u22652 runs. Target: th\u00eam VRAM/saturation/cold-warm \u22653 runs.\n\n## Security and migration notes\n\nCh\u1ec9 synthetic/de-identified corpus l\u00ean GLM; customer/\nrestricted data kh\u00f4ng ra cloud. Index signature ph\u00e2n bi\u1ec7t `glm-cloud` vs\n`vllm-local`; c\u1eaft sang vLLM = rebuild generation m\u1edbi. License tr\u01b0\u1edbc khi bundle\nlocal weights.\n\n## Out of scope\n\nAutoscaling; \u0111\u1ed5i desktop local-hash fallback m\u1eb7c \u0111\u1ecbnh.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 0 \u2014 Discovery & Gates" \
  --label "markhand-web,docs,web-p0,ready"

create_if_missing "P0-06 \u2014 Chunking, hybrid tuning v\u00e0 index signature" \
  --title "P0-06 \u2014 Chunking, hybrid tuning v\u00e0 index signature" \
  --body "## Metadata\n\n- Milestone: Phase 0 \u2014 Discovery & Gates\n- Phase code: 0\n- Issue ID: P0-06\n- Status: `blocked`\n- Catalog: `backlog/phase-0/issues/README.md`\n- Phase plan: `phase-0-discovery-and-gates.md`\n\n## Objective\n\nCh\u1ed1t chunking/hybrid parameters v\u00e0 canonical signature.\n\n## Implementation plan\n\nSo chunk sizes; FTS/vector/hybrid; tune RRF; \u0111\u1ecbnh ngh\u0129a length-delimited\nsignature g\u1ed3m model/revision/dim/normalize/chunk/text-normalization version;\nversion-aware identity v\u00e0 query modes current/as-of/compare/history; chu\u1ea9n ho\u00e1 typed\nclaim key/value/unit/scope/effective interval v\u00e0 deterministic numeric/enum/date/\nMUST-vs-MUST-NOT conflict candidates.\n\n## Files/modules\n\n`bench/markhand_web/retrieval/`, `expected-chunks.tsv`,\n`reports/retrieval-evaluation.md`, ADR index signature.\n\n## Dependencies / blocks\n\nDesktop baseline + embedding result.\n\n## Acceptance criteria\n\nSo s\u00e1nh identical candidates; source span v\u1eabn resolve; signature\ntest vector \u1ed5n \u0111\u1ecbnh; chunk ID c\u00f3 document-version; temporal/current accuracy v\u00e0\nversion-citation precision/recall \u0111\u1ea1t gate; conflict precision/recall, current-warning\nv\u00e0 resolved-history accuracy c\u00f3 cited evidence.\n\n## Required tests / evidence\n\nRecall/MRR/nDCG/citation/no-answer + variance; cross-run signature.\n\n## Security and migration notes\n\nSignature \u0111\u1ed5i t\u1ea1o index generation m\u1edbi, kh\u00f4ng tr\u1ed9n vector.\n\n## Out of scope\n\nServer adapters/ACL.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 0 \u2014 Discovery & Gates" \
  --label "markhand-web,docs,web-p0,blocked"

create_if_missing "P0-07 \u2014 PG/Qdrant target-scale topology" \
  --title "P0-07 \u2014 PG/Qdrant target-scale topology" \
  --body "## Metadata\n\n- Milestone: Phase 0 \u2014 Discovery & Gates\n- Phase code: 0\n- Issue ID: P0-07\n- Status: `blocked`\n- Catalog: `backlog/phase-0/issues/README.md`\n- Phase plan: `phase-0-discovery-and-gates.md`\n\n## Objective\n\nCh\u1ecdn Qdrant topology v\u00e0 PG partition strategy b\u1eb1ng mixed-load evidence.\n\n## Implementation plan\n\nGenerate realistic tenants; test max/org v\u00e0 aggregate production scale;\nshared/cohort collection; PG no-partition/hash; query+ingest+delete+snapshot.\n\n## Files/modules\n\n`bench/markhand_web/scale/`, `reports/scale-topology.md`, ADR Qdrant/PG.\n\n## Dependencies / blocks\n\nHardware/storage scale th\u1eadt; kh\u00f4ng ch\u1ea5p nh\u1eadn extrapolation nh\u1ecf.\n\n## Acceptance criteria\n\nFiltered P95/P99/recall \u0111\u1ea1t gate; ADR ghi measured limits; snapshot/\nrestore ch\u1ea1y \u0111\u01b0\u1ee3c.\n\n## Required tests / evidence\n\nLatency, throughput, quantized recall, RAM/disk, compaction,\nnoisy-neighbor, FTS.\n\n## Security and migration notes\n\nSynthetic tenant; m\u1ecdi query v\u1eabn c\u00f3 tenant filter.\n\n## Out of scope\n\nProduction RLS.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 0 \u2014 Discovery & Gates" \
  --label "markhand-web,docs,web-p0,blocked"

create_if_missing "P0-08 \u2014 Sizing converter v\u00e0 ingest backpressure" \
  --title "P0-08 \u2014 Sizing converter v\u00e0 ingest backpressure" \
  --body "## Metadata\n\n- Milestone: Phase 0 \u2014 Discovery & Gates\n- Phase code: 0\n- Issue ID: P0-08\n- Status: `blocked`\n- Catalog: `backlog/phase-0/issues/README.md`\n- Phase plan: `phase-0-discovery-and-gates.md`\n\n## Objective\n\nCh\u1ed1t worker count, limits, timeout, queue v\u00e0 recovery headroom.\n\n## Implementation plan\n\nBenchmark t\u1eebng format native/scan/audio; single/concurrent; CPU/RAM/temp;\nPDFium serialization; converter-vs-GPU bottleneck.\n\n## Files/modules\n\n`bench/markhand_web/ingest/`, `scripts/run_ingest_capacity.sh`,\n`reports/ingest-capacity.md`.\n\n## Dependencies / blocks\n\nGolden files + native deps + hardware.\n\n## Acceptance criteria\n\nM\u1ecdi POC format c\u00f3 sizing/timeout; \u226530% resource headroom; recovery\n2\u00d7 load kh\u00f4ng t\u0103ng queue v\u00f4 h\u1ea1n.\n\n## Required tests / evidence\n\nTime/file/page, throughput, peaks, timeout/failure, queue age.\n\n## Security and migration notes\n\nMalformed input ch\u1ec9 ch\u1ea1y d\u01b0\u1edbi limits.\n\n## Out of scope\n\nProduction job engine.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 0 \u2014 Discovery & Gates" \
  --label "markhand-web,docs,web-p0,blocked"

create_if_missing "P0-09 \u2014 Upload threat model, sandbox v\u00e0 license inventory" \
  --title "P0-09 \u2014 Upload threat model, sandbox v\u00e0 license inventory" \
  --body "## Metadata\n\n- Milestone: Phase 0 \u2014 Discovery & Gates\n- Phase code: 0\n- Issue ID: P0-09\n- Status: `blocked`\n- Catalog: `backlog/phase-0/issues/README.md`\n- Phase plan: `phase-0-discovery-and-gates.md`\n\n## Objective\n\nSecurity policy th\u1ef1c thi \u0111\u01b0\u1ee3c tr\u01b0\u1edbc khi nh\u1eadn upload.\n\n## Implementation plan\n\nThreat model spoof/bomb/parser/SSRF/exhaustion/traversal/injection/token/\nquota/tenant/compromised worker; ch\u1ed1t allowlist/limits/quarantine/sandbox; inventory\nsource/version/checksum/license.\n\n## Files/modules\n\n`docs/markhand-web-{upload-threat-model,upload-policy}.md`,\n`docs/markhand-web-model-license-inventory.md`, adversarial disposition YAML.\n\n## Dependencies / blocks\n\nP0-02/P0-08 evidence.\n\n## Acceptance criteria\n\nM\u1ed7i threat c\u00f3 prevention/detection/owner; sandbox non-root,\nread-only, no egress, resource/process/wall limits; unresolved model b\u1ecb exclude.\n\n## Required tests / evidence\n\nPolicy linter; sandbox blocks egress/traversal/fork/timeout.\n\n## Security and migration notes\n\nGLM policy theo data classification.\n\n## Out of scope\n\nProduction malware scanner.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 0 \u2014 Discovery & Gates" \
  --label "markhand-web,docs,web-p0,blocked"

create_if_missing "P0-10 \u2014 ADR, SLO/RPO/RTO v\u00e0 Phase 0 gate" \
  --title "P0-10 \u2014 ADR, SLO/RPO/RTO v\u00e0 Phase 0 gate" \
  --body "## Metadata\n\n- Milestone: Phase 0 \u2014 Discovery & Gates\n- Phase code: 0\n- Issue ID: P0-10\n- Status: `blocked`\n- Catalog: `backlog/phase-0/issues/README.md`\n- Phase plan: `phase-0-discovery-and-gates.md`\n\n## Objective\n\nChuy\u1ec3n evidence th\u00e0nh quy\u1ebft \u0111\u1ecbnh v\u00e0 clean restore proof.\n\n## Implementation plan\n\nADR document/artifact, tenancy/RLS, partition, Qdrant, auth/session,\nindex migration, backup order; ch\u1ed1t SLO; backup/restore ba h\u1ec7; close registry.\n\n## Files/modules\n\n`docs/adr/`, `docs/markhand-web-{sla-targets,risk-register}.md`,\n`bench/markhand_web/reports/restore-drill.md`.\n\n## Dependencies / blocks\n\nP0-01\u2026P0-09 + approvers.\n\n## Acceptance criteria\n\nM\u1ecdi decision \u0111\u01b0\u1ee3c duy\u1ec7t ho\u1eb7c block 1B; clean restore \u0111\u1ea1t RPO/RTO;\ngate link raw evidence; kh\u00f4ng high/critical/license issue thi\u1ebfu disposition.\n\n## Required tests / evidence\n\nIndependent gate rerun; component-loss restore; checksum/\nquery-ready/full-rebuild timing.\n\n## Security and migration notes\n\nPG authority; MinIO originals kh\u00f4ng reconstruct \u0111\u01b0\u1ee3c;\nmigration expand/cutover/contract.\n\n## Out of scope\n\nProduction HA v\u00e0 user onboarding.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 0 \u2014 Discovery & Gates" \
  --label "markhand-web,docs,web-p0,blocked"

create_if_missing "P1A-01 \u2014 Freeze desktop RAG v\u00e0 IPC contracts" \
  --title "P1A-01 \u2014 Freeze desktop RAG v\u00e0 IPC contracts" \
  --body "## Metadata\n\n- Milestone: Phase 1A \u2014 Knowledge Extraction\n- Phase code: 1A\n- Issue ID: P1A-01\n- Status: `done`\n- Catalog: `backlog/phase-1a/issues/README.md`\n- Phase plan: `phase-1a-knowledge-extraction.md`\n\n## Objective\n\nBaseline parity tr\u01b0\u1edbc khi move code.\n\n## Implementation plan\n\nInventory tests; fixtures top-k/score/snippet/anchor/answer/fallback/stats/\nincremental; canonical JSON cho 4 hybrid commands; offline + mock-provider flows.\n\n## Files/modules\n\n`app/src-tauri/src/{knowledge,vector_index}.rs`,\n`app/src/lib/{types,ipc}.ts`, backend/frontend contract fixtures.\n\n## Dependencies / blocks\n\nKh\u00f4ng.\n\n## Acceptance criteria\n\nCamelCase/answer modes/warnings/tolerance \u0111\u01b0\u1ee3c kh\u00f3a; undesirable\ncurrent behavior c\u0169ng \u0111\u01b0\u1ee3c ghi r\u00f5.\n\n## Security and migration notes\n\nSynthetic content/path, kh\u00f4ng credential.\n\n## Out of scope\n\nS\u1eeda ranking/concurrency.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1A \u2014 Knowledge Extraction" \
  --label "markhand-web,docs,web-p1a"

create_if_missing "P1A-02 \u2014 Populate knowledge skeleton v\u00e0 enforce dependency boundaries" \
  --title "P1A-02 \u2014 Populate knowledge skeleton v\u00e0 enforce dependency boundaries" \
  --body "## Metadata\n\n- Milestone: Phase 1A \u2014 Knowledge Extraction\n- Phase code: 1A\n- Issue ID: P1A-02\n- Status: `done`\n- Catalog: `backlog/phase-1a/issues/README.md`\n- Phase plan: `phase-1a-knowledge-extraction.md`\n\n## Objective\n\nHo\u00e0n thi\u1ec7n skeleton `crates/knowledge` do F-02 t\u1ea1o th\u00e0nh reusable\ncrate c\u00f3 typed errors v\u00e0 optional desktop features.\n\n## Implementation plan\n\nPopulate modules types/embedding/query/rank/citation/ask; features\n`desktop-sqlite`, `desktop-hnsw`; m\u1edf r\u1ed9ng CI deny-list theo boundary F-01. Kh\u00f4ng\nt\u1ea1o l\u1ea1i workspace member ho\u1eb7c convention.\n\n## Files/modules\n\n`Cargo.toml`, `crates/knowledge/**`, `.github/workflows/ci.yml`.\n\n## Dependencies / blocks\n\nBaseline committed.\n\n## Acceptance criteria\n\nBuild no-feature/all-feature; default tree kh\u00f4ng SQLite/HNSW; kh\u00f4ng\nTauri/axum/desktop; API kh\u00f4ng c\u00f3 DATA-root.\n\n## Security and migration notes\n\nMinimal dependency review.\n\n## Out of scope\n\nPG/Qdrant/server.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1A \u2014 Knowledge Extraction" \
  --label "markhand-web,docs,web-p1a"

create_if_missing "P1A-03 \u2014 Shared DTO v\u00e0 serde contract" \
  --title "P1A-03 \u2014 Shared DTO v\u00e0 serde contract" \
  --body "## Metadata\n\n- Milestone: Phase 1A \u2014 Knowledge Extraction\n- Phase code: 1A\n- Issue ID: P1A-03\n- Status: `done`\n- Catalog: `backlog/phase-1a/issues/README.md`\n- Phase plan: `phase-1a-knowledge-extraction.md`\n\n## Objective\n\nDi chuy\u1ec3n index/search/ask types m\u00e0 kh\u00f4ng \u0111\u1ed5i JSON.\n\n## Implementation plan\n\nIndex request/result/stats, hit/anchor/grounded answer/metadata; serde\nfixtures; temporary desktop re-export.\n\n## Files/modules\n\n`crates/knowledge/src/types.rs`, serde fixtures/tests,\n`app/src/lib/types.ts`.\n\n## Dependencies / blocks\n\nScaffold + frozen JSON.\n\n## Acceptance criteria\n\nCanonical JSON equivalent; no desktop path/state type; TypeScript\nkh\u00f4ng c\u1ea7n behavior change.\n\n## Security and migration notes\n\nErrors kh\u00f4ng expose provider secrets.\n\n## Out of scope\n\nOpenAPI generation.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1A \u2014 Knowledge Extraction" \
  --label "markhand-web,docs,web-p1a"

create_if_missing "P1A-04 \u2014 Durable identities v\u00e0 index signatures" \
  --title "P1A-04 \u2014 Durable identities v\u00e0 index signatures" \
  --body "## Metadata\n\n- Milestone: Phase 1A \u2014 Knowledge Extraction\n- Phase code: 1A\n- Issue ID: P1A-04\n- Status: `done`\n- Catalog: `backlog/phase-1a/issues/README.md`\n- Phase plan: `phase-1a-knowledge-extraction.md`\n\n## Objective\n\nDeterministic server identities, desktop compatibility.\n\n## Implementation plan\n\nVersioned length-delimited encoding; BLAKE3/SHA-256 document/chunk/index;\nsignature model/revision/dim/normalize/chunk/text version; fixed vectors; legacy\n`DefaultHasher` compatibility.\n\n## Files/modules\n\n`crates/knowledge/src/{identity,embedding}.rs`, identity fixtures.\n\n## Dependencies / blocks\n\nShared metadata; production values t\u1edbi t\u1eeb Phase 0.\n\n## Acceptance criteria\n\nCross-platform stable; no concatenation ambiguity; server kh\u00f4ng\nd\u00f9ng DefaultHasher; legacy index m\u1edf ho\u1eb7c explicit rebuild.\n\n## Security and migration notes\n\nHash l\u00e0 identity, kh\u00f4ng ph\u1ea3i access control; kh\u00f4ng mix version.\n\n## Out of scope\n\nCh\u1ecdn model.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1A \u2014 Knowledge Extraction" \
  --label "markhand-web,docs,web-p1a"

create_if_missing "P1A-05 \u2014 Query, local vectors v\u00e0 embedding plan" \
  --title "P1A-05 \u2014 Query, local vectors v\u00e0 embedding plan" \
  --body "## Metadata\n\n- Milestone: Phase 1A \u2014 Knowledge Extraction\n- Phase code: 1A\n- Issue ID: P1A-05\n- Status: `done`\n- Catalog: `backlog/phase-1a/issues/README.md`\n- Phase plan: `phase-1a-knowledge-extraction.md`\n\n## Objective\n\nT\u00e1ch pure query/embedding preparation.\n\n## Implementation plan\n\nNormalization, feature hash/vector norm, provider plan, dimension check,\nFTS escape; HTTP client v\u1eabn \u1edf core; gi\u1eef local fallback semantics.\n\n## Files/modules\n\n`crates/knowledge/src/{query,embedding}.rs`, tests; source desktop module.\n\n## Dependencies / blocks\n\nShared types.\n\n## Acceptance criteria\n\nOutput parity; query r\u1ed7ng/punctuation safe; mismatch/fallback kh\u00f4ng\n\u0111\u1ed5i; kh\u00f4ng Tauri/settings/filesystem.\n\n## Security and migration notes\n\nCredential-bearing URL kh\u00f4ng v\u00e0o signature/error.\n\n## Out of scope\n\nAsync client/new tokenizer.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1A \u2014 Knowledge Extraction" \
  --label "markhand-web,docs,web-p1a"

create_if_missing "P1A-06 \u2014 Rank, citation v\u00e0 grounded answer" \
  --title "P1A-06 \u2014 Rank, citation v\u00e0 grounded answer" \
  --body "## Metadata\n\n- Milestone: Phase 1A \u2014 Knowledge Extraction\n- Phase code: 1A\n- Issue ID: P1A-06\n- Status: `done`\n- Catalog: `backlog/phase-1a/issues/README.md`\n- Phase plan: `phase-1a-knowledge-extraction.md`\n\n## Objective\n\nReusable hybrid merge, anchors v\u00e0 grounding.\n\n## Implementation plan\n\nCosine/RRF/rerank/sort; snippet/page-slide-sheet anchor; extractive answer;\ncitation validator; separate LLM calls.\n\n## Files/modules\n\n`crates/knowledge/src/{rank,citation,ask}.rs`, golden tests.\n\n## Dependencies / blocks\n\nDTO/query.\n\n## Acceptance criteria\n\nTop-k/citation/answer parity trong tolerance; invented citation\nfallback; server caller kh\u00f4ng k\u00e9o desktop features.\n\n## Security and migration notes\n\nUntrusted passages kh\u00f4ng th\u00e0nh instruction.\n\n## Out of scope\n\nLearned reranker/streaming.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1A \u2014 Knowledge Extraction" \
  --label "markhand-web,docs,web-p1a"

create_if_missing "P1A-07 \u2014 SQLite desktop storage feature" \
  --title "P1A-07 \u2014 SQLite desktop storage feature" \
  --body "## Metadata\n\n- Milestone: Phase 1A \u2014 Knowledge Extraction\n- Phase code: 1A\n- Issue ID: P1A-07\n- Status: `done`\n- Catalog: `backlog/phase-1a/issues/README.md`\n- Phase plan: `phase-1a-knowledge-extraction.md`\n\n## Objective\n\nMove SQLite persistence, b\u1ecf reverse dependency v\u00e0o Tauri.\n\n## Implementation plan\n\nSchema/metadata/vector/incremental/FTS/hydration; API nh\u1eadn DB path +\ncaller-supplied corpus; Tauri gi\u1eef path jail/load.\n\n## Files/modules\n\n`crates/knowledge/src/desktop/sqlite.rs`, legacy DB fixture,\n`app/src-tauri/src/{knowledge,intelligence}.rs`.\n\n## Dependencies / blocks\n\nShared APIs stable.\n\n## Acceptance criteria\n\nLegacy DB parity; incremental/scope/signature/fallback gi\u1eef nguy\u00ean;\nkh\u00f4ng g\u1ecdi data_root/load_documents/resolve_within; optional rusqlite.\n\n## Security and migration notes\n\nCaller ch\u1ecbu path jail; schema additive ho\u1eb7c explicit rebuild.\n\n## Out of scope\n\nPostgreSQL/perf redesign.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1A \u2014 Knowledge Extraction" \
  --label "markhand-web,docs,web-p1a"

create_if_missing "P1A-08 \u2014 Persistent HNSW desktop feature" \
  --title "P1A-08 \u2014 Persistent HNSW desktop feature" \
  --body "## Metadata\n\n- Milestone: Phase 1A \u2014 Knowledge Extraction\n- Phase code: 1A\n- Issue ID: P1A-08\n- Status: `done`\n- Catalog: `backlog/phase-1a/issues/README.md`\n- Phase plan: `phase-1a-knowledge-extraction.md`\n\n## Objective\n\nMove optional ANN cache, SQLite v\u1eabn authority.\n\n## Implementation plan\n\nManifest/partition/rebuild/search/clear; legacy signature compatibility;\ncorrupt/mismatch fallback exact cosine.\n\n## Files/modules\n\n`crates/knowledge/src/desktop/hnsw.rs`, legacy HNSW fixture,\nsource `vector_index.rs`.\n\n## Dependencies / blocks\n\nFeature scaffold + vectors/identity.\n\n## Acceptance criteria\n\nRound-trip parity; corruption kh\u00f4ng m\u1ea5t data; location/threshold\nkh\u00f4ng \u0111\u1ed5i; `hnsw_rs` optional.\n\n## Security and migration notes\n\nValidate manifest bounds/path.\n\n## Out of scope\n\nHNSW tuning/Qdrant.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1A \u2014 Knowledge Extraction" \
  --label "markhand-web,docs,web-p1a"

create_if_missing "P1A-09 \u2014 Thin Tauri adapters" \
  --title "P1A-09 \u2014 Thin Tauri adapters" \
  --body "## Metadata\n\n- Milestone: Phase 1A \u2014 Knowledge Extraction\n- Phase code: 1A\n- Issue ID: P1A-09\n- Status: `done`\n- Catalog: `backlog/phase-1a/issues/README.md`\n- Phase plan: `phase-1a-knowledge-extraction.md`\n\n## Objective\n\nDesktop commands delegate shared crate, IPC gi\u1eef nguy\u00ean.\n\n## Implementation plan\n\nTauri gi\u1eef state/settings/path load/spawn_blocking/error mapping; delegate\nrebuild/stats/search/ask; retain legacy commands; remove duplicate only sau parity.\n\n## Files/modules\n\n`app/src-tauri/src/{knowledge,vector_index,intelligence,lib}.rs`,\nCargo manifests.\n\n## Dependencies / blocks\n\nPure logic + stores.\n\n## Acceptance criteria\n\nCommand/payload/result unchanged; source adapter m\u1ecfng; legacy index\nbehavior documented; no duplicate algorithm.\n\n## Security and migration notes\n\nPath jail v\u00e0 secret-safe errors gi\u1eef \u1edf desktop.\n\n## Out of scope\n\nUI/IPC rename/async redesign.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1A \u2014 Knowledge Extraction" \
  --label "markhand-web,docs,web-p1a"

create_if_missing "P1A-10 \u2014 CI parity v\u00e0 extraction gate" \
  --title "P1A-10 \u2014 CI parity v\u00e0 extraction gate" \
  --body "## Metadata\n\n- Milestone: Phase 1A \u2014 Knowledge Extraction\n- Phase code: 1A\n- Issue ID: P1A-10\n- Status: `done`\n- Catalog: `backlog/phase-1a/issues/README.md`\n- Phase plan: `phase-1a-knowledge-extraction.md`\n\n## Objective\n\nCh\u1ee9ng minh desktop equivalence v\u00e0 server usability.\n\n## Implementation plan\n\nFull feature/contract/golden matrix; no-feature server consumer test;\ndependency deny-list; docs compatibility; file perf/concurrency defects ri\u00eang.\n\n## Files/modules\n\nCI, `crates/knowledge/tests/`, desktop integration tests,\narchitecture/compatibility docs.\n\n## Dependencies / blocks\n\nAdapter cutover.\n\n## Acceptance criteria\n\nT\u1ea5t c\u1ea3 test xanh; golden trong tolerance; IPC unchanged; legacy\nindex path tested; server consumer kh\u00f4ng desktop deps.\n\n## Security and migration notes\n\nSynthetic fixtures; explicit index rebuild notice.\n\n## Out of scope\n\nServer/storage/auth.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1A \u2014 Knowledge Extraction" \
  --label "markhand-web,docs,web-p1a"

create_if_missing "P1B-F01 \u2014 Extend server skeleton v\u1edbi runtime POC" \
  --title "P1B-F01 \u2014 Extend server skeleton v\u1edbi runtime POC" \
  --body "## Metadata\n\n- Milestone: Phase 1B \u2014 Single-org POC\n- Phase code: 1B\n- Issue ID: P1B-F01\n- Status: `blocked`\n- Catalog: `backlog/phase-1b/issues/README.md`\n- Phase plan: `phase-1b-single-org-poc.md`\n\n## Implementation plan\n\nM\u1edf r\u1ed9ng `crates/server` API/worker skeleton t\u1eeb F-02/F-07 v\u1edbi runtime\ndependencies, application state, graceful shutdown v\u00e0 c\u00e1c config fields \u0111\u00e3 \u0111\u01b0\u1ee3c\nPhase 0 ph\u00ea duy\u1ec7t. Kh\u00f4ng t\u1ea1o l\u1ea1i workspace/config conventions.\n\n## Files/modules\n\n`crates/server/{Cargo.toml,src/{lib,main,config,error,state}.rs}`,\n`src/bin/worker.rs`.\n\n## Dependencies / blocks\n\nG0-ARCH.\n\n## Required tests / evidence\n\nAPI/worker compile \u0111\u1ed9c l\u1eadp; invalid URL/secret/limit/issuer/\nsignature kh\u00f4ng start; config/env/shutdown/table tests; secrets kh\u00f4ng `Debug`.\n\n## Security and migration notes\n\nUnsafe defaults ch\u1ec9 dev mode.\n\n## Out of scope\n\nbusiness routes/HA.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1B \u2014 Single-org POC" \
  --label "markhand-web,docs,web-p1b,blocked"

create_if_missing "P1B-F02 \u2014 POC deployment v\u00e0 isolation scaffold" \
  --title "P1B-F02 \u2014 POC deployment v\u00e0 isolation scaffold" \
  --body "## Metadata\n\n- Milestone: Phase 1B \u2014 Single-org POC\n- Phase code: 1B\n- Issue ID: P1B-F02\n- Status: `blocked`\n- Catalog: `backlog/phase-1b/issues/README.md`\n- Phase plan: `phase-1b-single-org-poc.md`\n\n## Implementation plan\n\nPinned API/converter/index images, compose services, health/init, non-root,\nread-only, tmpfs, dropped caps, converter no-egress, resource/secret limits.\n\n## Files/modules\n\n`deploy/{Dockerfile.server,Dockerfile.worker,compose.poc.yml,.env.example}`.\n\n## Dependencies / blocks\n\nF01 + G0-CAP/G0-SEC/G0-LIC.\n\n## Required tests / evidence\n\nClean host boot t\u1ef1 \u0111\u1ed9ng; API/worker image t\u00e1ch; isolation/\nUID/cap/egress/native format smoke tests.\n\n## Security and migration notes\n\nNarrow MinIO credentials, no bundled unlicensed model.\n**Out:** Kubernetes/HA.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1B \u2014 Single-org POC" \
  --label "markhand-web,docs,web-p1b,blocked"

create_if_missing "P1B-F03 \u2014 Multi-org-ready schema v\u00e0 immutable migrations" \
  --title "P1B-F03 \u2014 Multi-org-ready schema v\u00e0 immutable migrations" \
  --body "## Metadata\n\n- Milestone: Phase 1B \u2014 Single-org POC\n- Phase code: 1B\n- Issue ID: P1B-F03\n- Status: `blocked`\n- Catalog: `backlog/phase-1b/issues/README.md`\n- Phase plan: `phase-1b-single-org-poc.md`\n\n## Implementation plan\n\nMigrations org/auth/RBAC/groups/collections, immutable versions/artifacts,\natomic current-published pointer, parent/version/effective lineage, chunks/FTS,\nnormalized claims, conflict/evidence lifecycle, jobs/outbox, quota/audit/index;\nseed POC ri\u00eang.\n\n## Files/modules\n\n`crates/server/migrations/000*.sql`, `src/db/models.rs`.\n\n## Dependencies / blocks\n\nF01 + G0-ARCH.\n\n## Required tests / evidence\n\nM\u1ecdi business row c\u00f3 org; immutable versions; exactly one\ncurrent effective published version/logical document; concurrent publish/as-of/\nlineage checks; fresh + supported-upgrade migration/schema introspection.\n\n## Security and migration notes\n\nFiles immutable sau merge; RLS theo ADR.\n\n## Out of scope\n\ncustom role UI.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1B \u2014 Single-org POC" \
  --label "markhand-web,docs,web-p1b,blocked"

create_if_missing "P1B-F04 \u2014 OrgContext, repositories v\u00e0 state machine" \
  --title "P1B-F04 \u2014 OrgContext, repositories v\u00e0 state machine" \
  --body "## Metadata\n\n- Milestone: Phase 1B \u2014 Single-org POC\n- Phase code: 1B\n- Issue ID: P1B-F04\n- Status: `blocked`\n- Catalog: `backlog/phase-1b/issues/README.md`\n- Phase plan: `phase-1b-single-org-poc.md`\n\n## Implementation plan\n\nTenant-scoped repos, transaction helpers, legal document transitions;\ntransaction-local RLS context n\u1ebfu ch\u1ecdn.\n\n## Files/modules\n\n`src/auth/context.rs`, `src/db/{orgs,collections,documents,chunks}.rs`,\n`src/services/document_state.rs`.\n\n## Dependencies / blocks\n\nF03 + G0-ARCH.\n\n## Required tests / evidence\n\nKh\u00f4ng public business method thi\u1ebfu context; cross-org deny;\ninvalid/concurrent transition atomic; pool leakage test.\n\n## Security and migration notes\n\nEmpty scope fail closed.\n\n## Out of scope\n\nFull ACL semantics 1C.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1B \u2014 Single-org POC" \
  --label "markhand-web,docs,web-p1b,blocked"

create_if_missing "P1B-F05 \u2014 Password auth, rotating sessions v\u00e0 browser refresh transport" \
  --title "P1B-F05 \u2014 Password auth, rotating sessions v\u00e0 browser refresh transport" \
  --body "## Metadata\n\n- Milestone: Phase 1B \u2014 Single-org POC\n- Phase code: 1B\n- Issue ID: P1B-F05\n- Status: `blocked`\n- Catalog: `backlog/phase-1b/issues/README.md`\n- Phase plan: `phase-1b-single-org-poc.md`\n\n## Implementation plan\n\nArgon2; pinned JWT issuer/audience/alg/KID; short access; hashed rotating\nrefresh family; provider interface; POC guards/audit; ch\u1ed1t transport theo auth ADR.\nN\u1ebfu d\u00f9ng browser cookie: issue/rotate/clear `HttpOnly Secure SameSite`, CSRF token\nbinding + Origin validation v\u00e0 OpenAPI cookie contract.\n\n## Files/modules\n\n`src/auth/{password,jwt,session,provider,permissions,middleware}.rs`,\n`routes/auth.rs`.\n\n## Dependencies / blocks\n\nF03/F04 + auth ADR.\n\n## Required tests / evidence\n\nLogin/refresh/logout/me; reuse revokes family; disabled user\nblocked; alg/issuer/audience/expiry/race/permission/audit tests; cookie attributes,\nCSRF missing/mismatch, cross-origin refresh/logout v\u00e0 cookie clearing tests n\u1ebfu ADR\nch\u1ecdn cookie.\n\n## Security and migration notes\n\nNo token/password logs.\n\n## Out of scope\n\nOIDC/MFA/recovery.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1B \u2014 Single-org POC" \
  --label "markhand-web,docs,web-p1b,blocked"

create_if_missing "P1B-F06 \u2014 Fail-closed PG/Qdrant/MinIO adapters" \
  --title "P1B-F06 \u2014 Fail-closed PG/Qdrant/MinIO adapters" \
  --body "## Metadata\n\n- Milestone: Phase 1B \u2014 Single-org POC\n- Phase code: 1B\n- Issue ID: P1B-F06\n- Status: `blocked`\n- Catalog: `backlog/phase-1b/issues/README.md`\n- Phase plan: `phase-1b-single-org-poc.md`\n\n## Implementation plan\n\nPools, opaque key builder, quarantine/trusted namespace, deterministic\npoints, versioned collection, mandatory org/collection filters, typed errors.\n\n## Files/modules\n\n`src/storage/{keys,minio,qdrant}.rs`, `src/db/pool.rs`,\n`services/index_signature.rs`.\n\n## Dependencies / blocks\n\nF02/F04 + G0-ARCH/G0-RET/G1A.\n\n## Required tests / evidence\n\nMissing/empty filter rejected; no filename in key; payload has\nall identities; real-service contracts, traversal/fuzz, deterministic vectors.\n\n## Security and migration notes\n\nNo public key, least privilege.\n\n## Out of scope\n\ngeneric backend trait.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1B \u2014 Single-org POC" \
  --label "markhand-web,docs,web-p1b,blocked"

create_if_missing "P1B-I01 \u2014 Streaming quarantine upload validation" \
  --title "P1B-I01 \u2014 Streaming quarantine upload validation" \
  --body "## Metadata\n\n- Milestone: Phase 1B \u2014 Single-org POC\n- Phase code: 1B\n- Issue ID: P1B-I01\n- Status: `blocked`\n- Catalog: `backlog/phase-1b/issues/README.md`\n- Phase plan: `phase-1b-single-org-poc.md`\n\n## Implementation plan\n\nMultipart stream+hash; magic/extension canonical format; OOXML limits;\nPDF/audio limits; retention disposition.\n\n## Files/modules\n\n`routes/uploads.rs`, `services/upload/{stream,sniff,archive,limits}.rs`.\n\n## Dependencies / blocks\n\nF04/F06 + G0-SEC/G0-CAP.\n\n## Required tests / evidence\n\nSpoof/bomb/oversize/malformed/traversal/interruption rejected\nho\u1eb7c safely quarantined; bounded memory; adversarial/property tests.\n\n## Security and migration notes\n\nFilename metadata only.\n\n## Out of scope\n\nresumable upload/malware service.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1B \u2014 Single-org POC" \
  --label "markhand-web,docs,web-p1b,blocked"

create_if_missing "P1B-I02 \u2014 Atomic quota admission" \
  --title "P1B-I02 \u2014 Atomic quota admission" \
  --body "## Metadata\n\n- Milestone: Phase 1B \u2014 Single-org POC\n- Phase code: 1B\n- Issue ID: P1B-I02\n- Status: `blocked`\n- Catalog: `backlog/phase-1b/issues/README.md`\n- Phase plan: `phase-1b-single-org-poc.md`\n\n## Implementation plan\n\nTransactional reserve/finalize/refund, expiry, concurrent-job admission,\nquota headers/errors.\n\n## Files/modules\n\n`src/db/quota.rs`, `services/quota.rs`, quota middleware.\n\n## Dependencies / blocks\n\nF03/F04/I01 + G0-CAP.\n\n## Required tests / evidence\n\nConcurrent requests kh\u00f4ng over-reserve; every terminal path\nsettles; expiry/retry/crash/overflow tests.\n\n## Security and migration notes\n\nChecked arithmetic, client kh\u00f4ng s\u1eeda counter.\n\n## Out of scope\n\nbilling.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1B \u2014 Single-org POC" \
  --label "markhand-web,docs,web-p1b,blocked"

create_if_missing "P1B-I03 \u2014 Durable jobs, outbox v\u00e0 event log" \
  --title "P1B-I03 \u2014 Durable jobs, outbox v\u00e0 event log" \
  --body "## Metadata\n\n- Milestone: Phase 1B \u2014 Single-org POC\n- Phase code: 1B\n- Issue ID: P1B-I03\n- Status: `blocked`\n- Catalog: `backlog/phase-1b/issues/README.md`\n- Phase plan: `phase-1b-single-org-poc.md`\n\n## Implementation plan\n\nVersioned payload, transactional outbox, leased SKIP LOCKED claims,\nheartbeat/retry/checkpoint/cancel/dead-letter/idempotency/sequenced events.\n\n## Files/modules\n\n`src/jobs/**`, `src/db/jobs.rs`.\n\n## Dependencies / blocks\n\nF03/F04 + G0-CAP.\n\n## Required tests / evidence\n\nCommit/enqueue kh\u00f4ng split; lease reclaimed; duplicate harmless;\nkill/checkpoint/claim/dead-letter/cancel/outbox replay.\n\n## Security and migration notes\n\nIDs only, no content/secrets; backward-readable payloads.\n**Out:** Kafka/Redis queue.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1B \u2014 Single-org POC" \
  --label "markhand-web,docs,web-p1b,blocked"

create_if_missing "P1B-I04 \u2014 Isolated converter worker" \
  --title "P1B-I04 \u2014 Isolated converter worker" \
  --body "## Metadata\n\n- Milestone: Phase 1B \u2014 Single-org POC\n- Phase code: 1B\n- Issue ID: P1B-I04\n- Status: `blocked`\n- Catalog: `backlog/phase-1b/issues/README.md`\n- Phase plan: `phase-1b-single-org-poc.md`\n\n## Implementation plan\n\nDownload quarantine; materialize server-derived canonical extension;\nprocess/cgroup limits and kill descendants; ephemeral cleanup/heartbeat/cancel.\n\n## Files/modules\n\n`src/workers/{convert,sandbox,limits}.rs`, worker image/config.\n\n## Dependencies / blocks\n\nF02/I03 + G0-SEC/G0-CAP/G0-LIC.\n\n## Required tests / evidence\n\nNo network/host FS; timeout kills tree; cleanup all outcomes;\nfork/disk/RAM/malformed/cancel/all-format smoke.\n\n## Security and migration notes\n\nUnapproved model excluded, narrow credentials.\n\n## Out of scope\n\nVM sandbox.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1B \u2014 Single-org POC" \
  --label "markhand-web,docs,web-p1b,blocked"

create_if_missing "P1B-I05 \u2014 Idempotent conversion promotion saga" \
  --title "P1B-I05 \u2014 Idempotent conversion promotion saga" \
  --body "## Metadata\n\n- Milestone: Phase 1B \u2014 Single-org POC\n- Phase code: 1B\n- Issue ID: P1B-I05\n- Status: `blocked`\n- Catalog: `backlog/phase-1b/issues/README.md`\n- Phase plan: `phase-1b-single-org-poc.md`\n\n## Implementation plan\n\nCheckpoint download/convert/stage/promote/DB/cleanup; immutable version;\npublish/current pointer ri\u00eang v\u1edbi draft/latest upload; index outbox;\ncompensation/refund.\n\n## Files/modules\n\n`workers/convert.rs`, `services/{conversion,promotion,artifacts}.rs`,\n`db/document_versions.rs`.\n\n## Dependencies / blocks\n\nI01\u2013I04/F06/G1A.\n\n## Required tests / evidence\n\nRetry t\u1ea1o m\u1ed9t visible version/job; trusted ch\u1ec9 sau success;\nfault injection m\u1ecdi cross-store step; immutable checks.\n\n## Security and migration notes\n\nNever overwrite original; ACL inherited.\n\n## Out of scope\n\nuser merge.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1B \u2014 Single-org POC" \
  --label "markhand-web,docs,web-p1b,blocked"

create_if_missing "P1B-I06 \u2014 Chunk/embedding/index worker" \
  --title "P1B-I06 \u2014 Chunk/embedding/index worker" \
  --body "## Metadata\n\n- Milestone: Phase 1B \u2014 Single-org POC\n- Phase code: 1B\n- Issue ID: P1B-I06\n- Status: `blocked`\n- Catalog: `backlog/phase-1b/issues/README.md`\n- Phase plan: `phase-1b-single-org-poc.md`\n\n## Implementation plan\n\nCore chunking + knowledge identity/signature ch\u1ee9a `version_id`; PG\nchunks/FTS; separate embedding batches; Qdrant payload version/effective/current;\nextract typed claim key/value/unit/scope; incremental conflict candidate outbox;\nblocking client off async executor; deterministic upsert.\n\n## Files/modules\n\n`workers/{index,embedding}.rs`, `services/{chunking,embedding,indexing}.rs`.\n\n## Dependencies / blocks\n\nI03/I05/F06 + G0-RET/G0-CAP/G1A.\n\n## Required tests / evidence\n\nApproved signature; \u22641 replay batch; no duplicate; mismatch\nbefore publish; golden/mock/backpressure/kill/consistency tests.\n\n## Security and migration notes\n\nLocal approved embedding only; new signature=new generation.\n**Out:** user-selected models.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1B \u2014 Single-org POC" \
  --label "markhand-web,docs,web-p1b,blocked"

create_if_missing "P1B-I07 \u2014 Tombstone delete v\u00e0 reconcile" \
  --title "P1B-I07 \u2014 Tombstone delete v\u00e0 reconcile" \
  --body "## Metadata\n\n- Milestone: Phase 1B \u2014 Single-org POC\n- Phase code: 1B\n- Issue ID: P1B-I07\n- Status: `blocked`\n- Catalog: `backlog/phase-1b/issues/README.md`\n- Phase plan: `phase-1b-single-org-poc.md`\n\n## Implementation plan\n\nPG tombstone first; idempotent vector/object cleanup; dry-run/repair\nmissing/orphan/stale across three stores.\n\n## Files/modules\n\n`workers/{delete,reconcile}.rs`, `services/{deletion,reconciliation}.rs`.\n\n## Dependencies / blocks\n\nI03/I06/F06 + recovery ADR.\n\n## Required tests / evidence\n\nImmediate read suppression; drift safely repaired; repeated\nrepair, race, kill/resume matrix.\n\n## Security and migration notes\n\nScoped destructive audit.\n\n## Out of scope\n\nlegal hold/full ACL revoke.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1B \u2014 Single-org POC" \
  --label "markhand-web,docs,web-p1b,blocked"

create_if_missing "P1B-R01 \u2014 Tenant-scoped hybrid retrieval" \
  --title "P1B-R01 \u2014 Tenant-scoped hybrid retrieval" \
  --body "## Metadata\n\n- Milestone: Phase 1B \u2014 Single-org POC\n- Phase code: 1B\n- Issue ID: P1B-R01\n- Status: `blocked`\n- Catalog: `backlog/phase-1b/issues/README.md`\n- Phase plan: `phase-1b-single-org-poc.md`\n\n## Implementation plan\n\nResolve scope + current/as-of/compare/history mode; query embed; parallel\nQdrant/FTS v\u1edbi version filter; knowledge merge/rerank; PG hydration/recheck\nstate/ACL/version; hydrate only conflict evidence whose both sides remain authorized.\n\n## Files/modules\n\n`services/retrieval/{vector,fts,hydrate}.rs`, `db/search.rs`.\n\n## Dependencies / blocks\n\nF04/F06/I06 + G0-RET/G1A.\n\n## Required tests / evidence\n\nEmpty scope deny; stale vector no text; current kh\u00f4ng tr\u1ea3\nsuperseded version; as-of resolve \u0111\u00fang effective version; compare/history c\u00f9ng\nlineage; golden quality/cross-scope/deleted/one-leg outage/latency tests.\n\n## Security and migration notes\n\nText only after authorized hydration.\n\n## Out of scope\n\nnew reranker.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1B \u2014 Single-org POC" \
  --label "markhand-web,docs,web-p1b,blocked"

create_if_missing "P1B-R02 \u2014 Citation, preview v\u00e0 download authorization" \
  --title "P1B-R02 \u2014 Citation, preview v\u00e0 download authorization" \
  --body "## Metadata\n\n- Milestone: Phase 1B \u2014 Single-org POC\n- Phase code: 1B\n- Issue ID: P1B-R02\n- Status: `blocked`\n- Catalog: `backlog/phase-1b/issues/README.md`\n- Phase plan: `phase-1b-single-org-poc.md`\n\n## Implementation plan\n\nStable anchor pin logical document/version number/version ID/content hash/\neffective time/current flag; fresh auth per resolve; trusted Markdown fetch; short\nsingle-purpose download capability.\n\n## Files/modules\n\n`services/{citation,preview,download}.rs`, document routes.\n\n## Dependencies / blocks\n\nF05/F06/R01.\n\n## Required tests / evidence\n\nQuote/hash/version/anchor valid; historical permission + fresh\nACL; delete/suspend/removal deny; IDOR, expiry/replay, multi-document/multi-version,\nPDF/PPTX/XLSX anchor tests.\n\n## Security and migration notes\n\nNo raw bucket credential/key.\n\n## Out of scope\n\nrich rendering.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1B \u2014 Single-org POC" \
  --label "markhand-web,docs,web-p1b,blocked"

create_if_missing "P1B-R03 \u2014 Grounded Q&A, stream v\u00e0 fallback" \
  --title "P1B-R03 \u2014 Grounded Q&A, stream v\u00e0 fallback" \
  --body "## Metadata\n\n- Milestone: Phase 1B \u2014 Single-org POC\n- Phase code: 1B\n- Issue ID: P1B-R03\n- Status: `blocked`\n- Catalog: `backlog/phase-1b/issues/README.md`\n- Phase plan: `phase-1b-single-org-poc.md`\n\n## Implementation plan\n\nPolicy-separated prompt, untrusted passage framing, GLM, version-aware\ncitation validation, current answer + history/change note, token stream,\ncurrent unresolved-conflict warnings + resolved-history note, token stream,\ndeterministic extractive fallback.\n\n## Files/modules\n\n`services/qa/{prompt,provider,grounding,stream}.rs`.\n\n## Dependencies / blocks\n\nR01/R02 + G0-RET/G0-SEC/G1A.\n\n## Required tests / evidence\n\nCitation subset only; current claim kh\u00f4ng cite version c\u0169;\ncompare cite old+new v\u00e0 \u0111\u00fang delta; injection kh\u00f4ng tool/scope change; provider\noutage fallback; BA/design numeric conflict warning v\u00e0 v2 resolution; false-positive/\naccepted-exception; fabricated/version-mix/conflict citation, timeout,\ndelete-during-stream tests.\n\n## Security and migration notes\n\nAudit metadata only.\n\n## Out of scope\n\nagents/memory/web browse.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1B \u2014 Single-org POC" \
  --label "markhand-web,docs,web-p1b,blocked"

create_if_missing "P1B-R04 \u2014 Collection/document/job REST API" \
  --title "P1B-R04 \u2014 Collection/document/job REST API" \
  --body "## Metadata\n\n- Milestone: Phase 1B \u2014 Single-org POC\n- Phase code: 1B\n- Issue ID: P1B-R04\n- Status: `blocked`\n- Catalog: `backlog/phase-1b/issues/README.md`\n- Phase plan: `phase-1b-single-org-poc.md`\n\n## Implementation plan\n\n`/api/v1` collection POC; upload/list/get/preview/delete/reindex; immutable\nversion list/get/diff/current publish; conflict list/detail/triage + evidence routes;\njob status; pagination/idempotency/error schema.\n\n## Files/modules\n\n`routes/{collections,documents,jobs}.rs`, `api/{types,error,pagination}.rs`.\n\n## Dependencies / blocks\n\nF04/F05/I01/I03/I07/R02.\n\n## Required tests / evidence\n\nOrg context + permissions; stable errors; idempotent reindex;\nHTTP contract/pagination/IDOR/malformed tests.\n\n## Security and migration notes\n\nBounded body/page, no internals.\n\n## Out of scope\n\nadmin membership API.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1B \u2014 Single-org POC" \
  --label "markhand-web,docs,web-p1b,blocked"

create_if_missing "P1B-R05 \u2014 Search/ask/resumable SSE API" \
  --title "P1B-R05 \u2014 Search/ask/resumable SSE API" \
  --body "## Metadata\n\n- Milestone: Phase 1B \u2014 Single-org POC\n- Phase code: 1B\n- Issue ID: P1B-R05\n- Status: `blocked`\n- Catalog: `backlog/phase-1b/issues/README.md`\n- Phase plan: `phase-1b-single-org-poc.md`\n\n## Implementation plan\n\nSearch/ask/stream routes; versioned sequence; Last-Event-ID replay;\nheartbeat/bounded buffering; auth expiry/revoke close.\n\n## Files/modules\n\n`routes/{search,ask,events}.rs`, `api/sse.rs`.\n\n## Dependencies / blocks\n\nF05/I03/R01/R03/R04.\n\n## Required tests / evidence\n\nNo lost acknowledged/duplicate sequence; bounded slow client;\nreconnect/order/expiry/revoke/worker restart.\n\n## Security and migration notes\n\nScoped per user/org/job, no cache.\n\n## Out of scope\n\nWebSocket.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1B \u2014 Single-org POC" \
  --label "markhand-web,docs,web-p1b,blocked"

create_if_missing "P1B-R06 \u2014 OpenAPI, rate limit v\u00e0 readiness" \
  --title "P1B-R06 \u2014 OpenAPI, rate limit v\u00e0 readiness" \
  --body "## Metadata\n\n- Milestone: Phase 1B \u2014 Single-org POC\n- Phase code: 1B\n- Issue ID: P1B-R06\n- Status: `blocked`\n- Catalog: `backlog/phase-1b/issues/README.md`\n- Phase plan: `phase-1b-single-org-poc.md`\n\n## Implementation plan\n\nComplete OpenAPI/fixtures; request IDs; CORS; IP auth/user limits; quota\nmetadata; live/ready/start checks.\n\n## Files/modules\n\n`api/openapi.rs`, OpenAPI YAML, middleware, `routes/health.rs`.\n\n## Dependencies / blocks\n\nR04/R05/F05 + G0-SLO.\n\n## Required tests / evidence\n\nEvery route represented; readiness detects required deps/\nsignature/reconciliation; 429 metadata; snapshots/rate/trusted-proxy/outage tests.\n\n## Security and migration notes\n\nConservative CORS/proxy trust.\n\n## Out of scope\n\ndistributed limiter.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1B \u2014 Single-org POC" \
  --label "markhand-web,docs,web-p1b,blocked"

create_if_missing "P1B-O01 \u2014 End-to-end telemetry v\u00e0 safe audit" \
  --title "P1B-O01 \u2014 End-to-end telemetry v\u00e0 safe audit" \
  --body "## Metadata\n\n- Milestone: Phase 1B \u2014 Single-org POC\n- Phase code: 1B\n- Issue ID: P1B-O01\n- Status: `blocked`\n- Catalog: `backlog/phase-1b/issues/README.md`\n- Phase plan: `phase-1b-single-org-poc.md`\n\n## Implementation plan\n\nTraces API\u2192jobs\u2192convert/embed/retrieval/GLM; latency/queue/conversion/\nembedding/retrieval/drift/quota/backup metrics; append-only audit.\n\n## Files/modules\n\n`src/telemetry/**`, `services/audit.rs`, `db/audit.rs`, OTel config.\n\n## Dependencies / blocks\n\nF01/F05/I03 + G0-SLO.\n\n## Required tests / evidence\n\nCorrelation qua async; action/deny coverage; canary secret/\ncontent absent; trace/cardinality/redaction/audit tests.\n\n## Security and migration notes\n\nAllowlist log fields.\n\n## Out of scope\n\nSIEM.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1B \u2014 Single-org POC" \
  --label "markhand-web,docs,web-p1b,blocked"

create_if_missing "P1B-O02 \u2014 Dashboards, alerts v\u00e0 runbooks" \
  --title "P1B-O02 \u2014 Dashboards, alerts v\u00e0 runbooks" \
  --body "## Metadata\n\n- Milestone: Phase 1B \u2014 Single-org POC\n- Phase code: 1B\n- Issue ID: P1B-O02\n- Status: `blocked`\n- Catalog: `backlog/phase-1b/issues/README.md`\n- Phase plan: `phase-1b-single-org-poc.md`\n\n## Implementation plan\n\nSLO/queue/disk/dependency alerts; runbooks jobs/parser/outage/rebuild/disk/\nGLM/key rotation.\n\n## Files/modules\n\n`deploy/observability/**`, `docs/runbooks/**`.\n\n## Dependencies / blocks\n\nF02/F06/I03/O01 + G0-SLO.\n\n## Required tests / evidence\n\nTrigger t\u1eebng alert; runbook detection\u2192contain\u2192recover\u2192verify;\nrule validation/fault/tabletop evidence.\n\n## Security and migration notes\n\nNo tenant/document high-cardinality labels.\n\n## Out of scope\n\nstaffing.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1B \u2014 Single-org POC" \
  --label "markhand-web,docs,web-p1b,blocked"

create_if_missing "P1B-O03 \u2014 Backup/restore v\u00e0 migration safety" \
  --title "P1B-O03 \u2014 Backup/restore v\u00e0 migration safety" \
  --body "## Metadata\n\n- Milestone: Phase 1B \u2014 Single-org POC\n- Phase code: 1B\n- Issue ID: P1B-O03\n- Status: `blocked`\n- Catalog: `backlog/phase-1b/issues/README.md`\n- Phase plan: `phase-1b-single-org-poc.md`\n\n## Implementation plan\n\nPG PITR, MinIO version inventory, Qdrant snapshot, consistency fence/\nmanifest, restore order, reconcile-before-ready, vector rebuild.\n\n## Files/modules\n\n`deploy/backup/**`, restore/migration runbooks, restore guard.\n\n## Dependencies / blocks\n\nF02/F03/F06/I07 + G0-ARCH/G0-SLO.\n\n## Required tests / evidence\n\nClean restore \u0111\u1ea1t RPO/RTO; missing/orphan detect; readiness\nfalse until reconcile; PG rebuild; corrupt manifest/upgrade tests.\n\n## Security and migration notes\n\nEncrypted narrow credentials; expand/cutover/contract.\n**Out:** multi-region DR.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1B \u2014 Single-org POC" \
  --label "markhand-web,docs,web-p1b,blocked"

create_if_missing "P1B-O04 \u2014 Vertical-slice/security release suite" \
  --title "P1B-O04 \u2014 Vertical-slice/security release suite" \
  --body "## Metadata\n\n- Milestone: Phase 1B \u2014 Single-org POC\n- Phase code: 1B\n- Issue ID: P1B-O04\n- Status: `blocked`\n- Catalog: `backlog/phase-1b/issues/README.md`\n- Phase plan: `phase-1b-single-org-poc.md`\n\n## Implementation plan\n\nClean stack, seed org/accounts; every format upload\u2192citation; suspend/\nmembership remove/delete; adversarial + fault injection.\n\n## Files/modules\n\n`crates/server/tests/e2e/**`, POC manifest, deploy test script.\n\n## Dependencies / blocks\n\nF01\u2013R06 + G0-SEC/G1A.\n\n## Required tests / evidence\n\nAll formats pass; unauthorized gets no text; malicious\nrejected/contained; worker kill consistent; evidence redacted.\n\n## Security and migration notes\n\nHigh/critical blocks release.\n\n## Out of scope\n\nfull 1C matrix.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1B \u2014 Single-org POC" \
  --label "markhand-web,docs,web-p1b,blocked"

create_if_missing "P1B-O05 \u2014 Mixed-load soak v\u00e0 POC qualification" \
  --title "P1B-O05 \u2014 Mixed-load soak v\u00e0 POC qualification" \
  --body "## Metadata\n\n- Milestone: Phase 1B \u2014 Single-org POC\n- Phase code: 1B\n- Issue ID: P1B-O05\n- Status: `blocked`\n- Catalog: `backlog/phase-1b/issues/README.md`\n- Phase plan: `phase-1b-single-org-poc.md`\n\n## Implementation plan\n\nIngest/query/delete/reconcile mixed load + failures; monitor leaks/queue;\nrestore; aggregate gate report.\n\n## Files/modules\n\n`bench/markhand_web/{soak,workloads,reports/phase-1b-gate}*`.\n\n## Dependencies / blocks\n\nO02/O03/O04 + G0-CAP/G0-SLO.\n\n## Required tests / evidence\n\nNumeric gates pass; no unbounded memory/temp/connection/queue;\nrecovery/worker/dependency/restore/post-restore retrieval evidence.\n\n## Security and migration notes\n\nSynthetic/redacted, exact versions recorded.\n**Out:** production/multi-org.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1B \u2014 Single-org POC" \
  --label "markhand-web,docs,web-p1b,blocked"

create_if_missing "1C-01 \u2014 Organization lifecycle v\u00e0 validated context" \
  --title "1C-01 \u2014 Organization lifecycle v\u00e0 validated context" \
  --body "## Metadata\n\n- Milestone: Phase 1C \u2014 Multi-org Security\n- Phase code: 1C\n- Issue ID: 1C-01\n- Status: `backlog`\n- Catalog: `backlog/phase-1c/issues/README.md`\n- Phase plan: `phase-1c-multi-org-security.md`\n\n## Dependencies / blocks\n\nPhase 1B auth/schema.\nforged/stale header deny; two-org resolver/integration tests.\n\n## Required tests / evidence\n\nCh\u1ec9 th\u1ea5y org c\u1ee7a m\u00ecnh;\n\n## Security and migration notes\n\nKh\u00f4ng global org state; audit switch.\n\n## Out of scope\n\nbilling/OIDC.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1C \u2014 Multi-org Security" \
  --label "markhand-web,docs,web-p1c,backlog"

create_if_missing "1C-02 \u2014 Membership, invites v\u00e0 last-owner invariant" \
  --title "1C-02 \u2014 Membership, invites v\u00e0 last-owner invariant" \
  --body "## Metadata\n\n- Milestone: Phase 1C \u2014 Multi-org Security\n- Phase code: 1C\n- Issue ID: 1C-02\n- Status: `backlog`\n- Catalog: `backlog/phase-1c/issues/README.md`\n- Phase plan: `phase-1c-multi-org-security.md`\n\n## Dependencies / blocks\n\n1C-01.\nkh\u00f4ng qu\u1ea3n owner; concurrent owner removal, invite replay/expiry, escalation tests.\n\n## Required tests / evidence\n\nKh\u00f4ng remove/downgrade last owner; admin\n\n## Security and migration notes\n\nRow lock, expand/backfill version; plaintext invite kh\u00f4ng\nl\u01b0u DB/log. **Out:** automated email delivery/SCIM/MFA.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1C \u2014 Multi-org Security" \
  --label "markhand-web,docs,web-p1c,backlog"

create_if_missing "1C-03 \u2014 Canonical RBAC seed" \
  --title "1C-03 \u2014 Canonical RBAC seed" \
  --body "## Metadata\n\n- Milestone: Phase 1C \u2014 Multi-org Security\n- Phase code: 1C\n- Issue ID: 1C-03\n- Status: `backlog`\n- Catalog: `backlog/phase-1c/issues/README.md`\n- Phase plan: `phase-1c-multi-org-security.md`\n\n## Dependencies / blocks\n\nPhase 1B role schema.\nduplicate/missing/immutable mutation tests; UI kh\u00f4ng hard-code matrix.\n\n## Required tests / evidence\n\nMatrix \u0111\u00fang/idempotent,\n\n## Security and migration notes\n\nStable keys, expand/backfill.\n\n## Out of scope\n\ncustom role builder.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1C \u2014 Multi-org Security" \
  --label "markhand-web,docs,web-p1c,backlog"

create_if_missing "1C-04 \u2014 Route/service guards v\u00e0 service identities" \
  --title "1C-04 \u2014 Route/service guards v\u00e0 service identities" \
  --body "## Metadata\n\n- Milestone: Phase 1C \u2014 Multi-org Security\n- Phase code: 1C\n- Issue ID: 1C-04\n- Status: `backlog`\n- Catalog: `backlog/phase-1c/issues/README.md`\n- Phase plan: `phase-1c-multi-org-security.md`\n\n## Dependencies / blocks\n\n1C-01/03.\nmissing-guard inventory, direct-service v\u00e0 worker misuse tests.\n\n## Required tests / evidence\n\nAllow/deny m\u1ed7i permission c\u1ea3 hai layer;\n\n## Security and migration notes\n\nKh\u00f4ng `internal=true` bypass.\n\n## Out of scope\n\ngeneric ABAC.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1C \u2014 Multi-org Security" \
  --label "markhand-web,docs,web-p1c,backlog"

create_if_missing "1C-05 \u2014 Collection ACL resolver/cache" \
  --title "1C-05 \u2014 Collection ACL resolver/cache" \
  --body "## Metadata\n\n- Milestone: Phase 1C \u2014 Multi-org Security\n- Phase code: 1C\n- Issue ID: 1C-05\n- Status: `backlog`\n- Catalog: `backlog/phase-1c/issues/README.md`\n- Phase plan: `phase-1c-multi-org-security.md`\n\n## Dependencies / blocks\n\n1C-02/03.\ngrants/status/cache/revoke tests.\n\n## Required tests / evidence\n\nSemantics \u0111\u00fang, empty/error fail closed;\n\n## Security and migration notes\n\nBackfill ACL version.\n\n## Out of scope\n\nnested/time-based groups.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1C \u2014 Multi-org Security" \
  --label "markhand-web,docs,web-p1c,backlog"

create_if_missing "1C-06 \u2014 PostgreSQL ACL enforcement" \
  --title "1C-06 \u2014 PostgreSQL ACL enforcement" \
  --body "## Metadata\n\n- Milestone: Phase 1C \u2014 Multi-org Security\n- Phase code: 1C\n- Issue ID: 1C-06\n- Status: `backlog`\n- Catalog: `backlog/phase-1c/issues/README.md`\n- Phase plan: `phase-1c-multi-org-security.md`\n\n## Dependencies / blocks\n\n1C-05.\nleak; SQL join/subquery/missing-predicate tests.\n\n## Required tests / evidence\n\nKh\u00f4ng path thi\u1ebfu context; no existence/count\n\n## Security and migration notes\n\nPG authority, prepared queries.\n\n## Out of scope\n\nvector/object path.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1C \u2014 Multi-org Security" \
  --label "markhand-web,docs,web-p1c,backlog"

create_if_missing "1C-07 \u2014 Qdrant/storage/jobs fail-closed enforcement" \
  --title "1C-07 \u2014 Qdrant/storage/jobs fail-closed enforcement" \
  --body "## Metadata\n\n- Milestone: Phase 1C \u2014 Multi-org Security\n- Phase code: 1C\n- Issue ID: 1C-07\n- Status: `backlog`\n- Catalog: `backlog/phase-1c/issues/README.md`\n- Phase plan: `phase-1c-multi-org-security.md`\n\n## Dependencies / blocks\n\n1C-05/06.\nQdrant failure, forged payload, job ID, stream revoke, signed URL replay tests.\n\n## Required tests / evidence\n\nMissing/malformed/timeout/mismatch deny;\n\n## Security and migration notes\n\nNo signed URL logs.\n\n## Out of scope\n\npublic sharing/CDN.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1C \u2014 Multi-org Security" \
  --label "markhand-web,docs,web-p1c,backlog"

create_if_missing "1C-08 \u2014 RLS v\u00e0 pool defense" \
  --title "1C-08 \u2014 RLS v\u00e0 pool defense" \
  --body "## Metadata\n\n- Milestone: Phase 1C \u2014 Multi-org Security\n- Phase code: 1C\n- Issue ID: 1C-08\n- Status: `backlog`\n- Catalog: `backlog/phase-1c/issues/README.md`\n- Phase plan: `phase-1c-multi-org-security.md`\n\n## Dependencies / blocks\n\nADR + 1C-01/06.\npooled-context/worker misuse/migration tests.\n\n## Required tests / evidence\n\nNo owner/BYPASSRLS; wrong/missing/\n\n## Security and migration notes\n\nExpand policy tr\u01b0\u1edbc force; n\u1ebfu kh\u00f4ng ch\u1ecdn, close b\u1eb1ng ADR\n+ repository evidence. **Out:** thay app guards b\u1eb1ng RLS.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1C \u2014 Multi-org Security" \
  --label "markhand-web,docs,web-p1c,backlog"

create_if_missing "1C-09 \u2014 Atomic quota lifecycle" \
  --title "1C-09 \u2014 Atomic quota lifecycle" \
  --body "## Metadata\n\n- Milestone: Phase 1C \u2014 Multi-org Security\n- Phase code: 1C\n- Issue ID: 1C-09\n- Status: `backlog`\n- Catalog: `backlog/phase-1c/issues/README.md`\n- Phase plan: `phase-1c-multi-org-security.md`\n\n## Dependencies / blocks\n\nPhase 1B jobs + 1C-01.\nkh\u00f4ng over-limit; crash/retry/cancel/timeout/actual-usage tests.\n\n## Required tests / evidence\n\n100 concurrent reservations\n\n## Security and migration notes\n\nChecked arithmetic, org/resource unique key.\n\n## Out of scope\n\nbilling.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1C \u2014 Multi-org Security" \
  --label "markhand-web,docs,web-p1c,backlog"

create_if_missing "1C-10 \u2014 Rate limit v\u00e0 per-org fairness" \
  --title "1C-10 \u2014 Rate limit v\u00e0 per-org fairness" \
  --body "## Metadata\n\n- Milestone: Phase 1C \u2014 Multi-org Security\n- Phase code: 1C\n- Issue ID: 1C-10\n- Status: `backlog`\n- Catalog: `backlog/phase-1c/issues/README.md`\n- Phase plan: `phase-1c-multi-org-security.md`\n\n## Dependencies / blocks\n\n1C-09 + Phase 0 SLO/capacity.\nSLO org kh\u00e1c; burst/window/fair-load/crash-release/proxy tests.\n\n## Required tests / evidence\n\nNoisy org kh\u00f4ng ph\u00e1\n\n## Security and migration notes\n\nCh\u1ec9 trusted proxy IP, bounded state.\n\n## Out of scope\n\nmulti-region.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1C \u2014 Multi-org Security" \
  --label "markhand-web,docs,web-p1c,backlog"

create_if_missing "1C-11 \u2014 Audit/admin APIs" \
  --title "1C-11 \u2014 Audit/admin APIs" \
  --body "## Metadata\n\n- Milestone: Phase 1C \u2014 Multi-org Security\n- Phase code: 1C\n- Issue ID: 1C-11\n- Status: `backlog`\n- Catalog: `backlog/phase-1c/issues/README.md`\n- Phase plan: `phase-1c-multi-org-security.md`\n\n## Dependencies / blocks\n\n1C-02\u202610.\nresult/request ID; coverage/access/pagination/redaction/retention tests.\n\n## Required tests / evidence\n\nM\u1ecdi mutation c\u00f3 actor/org/action/target/\n\n## Security and migration notes\n\nNo document/prompt/token/PII/URL.\n\n## Out of scope\n\nSIEM archive.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1C \u2014 Multi-org Security" \
  --label "markhand-web,docs,web-p1c,backlog"

create_if_missing "1C-12 \u2014 Multi-org denial suite" \
  --title "1C-12 \u2014 Multi-org denial suite" \
  --body "## Metadata\n\n- Milestone: Phase 1C \u2014 Multi-org Security\n- Phase code: 1C\n- Issue ID: 1C-12\n- Status: `backlog`\n- Catalog: `backlog/phase-1c/issues/README.md`\n- Phase plan: `phase-1c-multi-org-security.md`\n\n## Dependencies / blocks\n\n1C-01\u202611.\nroute + direct service, CI + deployed environment.\n\n## Required tests / evidence\n\nZero content/metadata/existence leak,\n\n## Security and migration notes\n\nDeployment-like roles, exploit-first regression.\n**Out:** external pentest.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1C \u2014 Multi-org Security" \
  --label "markhand-web,docs,web-p1c,backlog"

create_if_missing "1C-13 \u2014 Security/revoke/load gate" \
  --title "1C-13 \u2014 Security/revoke/load gate" \
  --body "## Metadata\n\n- Milestone: Phase 1C \u2014 Multi-org Security\n- Phase code: 1C\n- Issue ID: 1C-13\n- Status: `backlog`\n- Catalog: `backlog/phase-1c/issues/README.md`\n- Phase plan: `phase-1c-multi-org-security.md`\n\n## Dependencies / blocks\n\n1C-10/11/12.\nfairness SLO; audit complete; no undispositioned high/critical.\n\n## Required tests / evidence\n\nLeakage 0; revoke bound; quota recovery;\n\n## Security and migration notes\n\nRecord environment/threshold/approver.\n\n## Out of scope\n\nSPA/OIDC.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 1C \u2014 Multi-org Security" \
  --label "markhand-web,docs,web-p1c,backlog"

create_if_missing "P2-01 \u2014 React/Vite workspace v\u00e0 UI foundations" \
  --title "P2-01 \u2014 React/Vite workspace v\u00e0 UI foundations" \
  --body "## Metadata\n\n- Milestone: Phase 2 \u2014 Web SPA MVP\n- Phase code: 2\n- Issue ID: P2-01\n- Status: `backlog`\n- Catalog: `backlog/phase-2/issues/README.md`\n- Phase plan: `phase-2-web-spa.md`\n\n## Dependencies / blocks\n\nKh\u00f4ng.\ntypecheck/lint/unit/dependency-boundary; desktop v\u1eabn xanh.\n\n## Required tests / evidence\n\nBuild/test \u0111\u1ed9c l\u1eadp; no Tauri import;\n\n## Security and migration notes\n\nDependency/license scan.\n\n## Out of scope\n\nshared package/redesign desktop.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 2 \u2014 Web SPA MVP" \
  --label "markhand-web,docs,web-p2,backlog"

create_if_missing "P2-02 \u2014 OpenAPI contracts v\u00e0 mock server" \
  --title "P2-02 \u2014 OpenAPI contracts v\u00e0 mock server" \
  --body "## Metadata\n\n- Milestone: Phase 2 \u2014 Web SPA MVP\n- Phase code: 2\n- Issue ID: P2-02\n- Status: `backlog`\n- Catalog: `backlog/phase-2/issues/README.md`\n- Phase plan: `phase-2-web-spa.md`\n\n## Dependencies / blocks\n\nStable 1B OpenAPI.\nimmutable; fixture/schema/breaking-change tests; mock excluded production.\n\n## Required tests / evidence\n\nDrift fails CI; generated files\n\n## Security and migration notes\n\nN/A \u2014 kh\u00f4ng thay \u0111\u1ed5i persisted schema; fixtures synthetic,\nkh\u00f4ng ch\u1ee9a token/PII th\u1eadt.\n\n## Out of scope\n\nCh\u1edd to\u00e0n b\u1ed9 1C m\u1edbi l\u00e0m UI.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 2 \u2014 Web SPA MVP" \
  --label "markhand-web,docs,web-p2,backlog"

create_if_missing "P2-03 \u2014 Typed HTTP client/session refresh" \
  --title "P2-03 \u2014 Typed HTTP client/session refresh" \
  --body "## Metadata\n\n- Milestone: Phase 2 \u2014 Web SPA MVP\n- Phase code: 2\n- Issue ID: P2-03\n- Status: `backlog`\n- Catalog: `backlog/phase-2/issues/README.md`\n- Phase plan: `phase-2-web-spa.md`\n\n## Dependencies / blocks\n\nP2-02.\nlogout; race/loop/malformed/403/429/network/abort tests.\n\n## Required tests / evidence\n\nConcurrent 401 m\u1ed9t refresh; revoked refresh\n\n## Security and migration notes\n\nNo token storage/log.\n\n## Out of scope\n\noffline queue/Tauri IPC.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 2 \u2014 Web SPA MVP" \
  --label "markhand-web,docs,web-p2,backlog"

create_if_missing "P2-04 \u2014 Fetch-based SSE transport" \
  --title "P2-04 \u2014 Fetch-based SSE transport" \
  --body "## Metadata\n\n- Milestone: Phase 2 \u2014 Web SPA MVP\n- Phase code: 2\n- Issue ID: P2-04\n- Status: `backlog`\n- Catalog: `backlog/phase-2/issues/README.md`\n- Phase plan: `phase-2-web-spa.md`\n\n## Dependencies / blocks\n\nP2-02/03.\nchunk boundary/reconnect/order/revoke/cancel tests.\n\n## Required tests / evidence\n\nKh\u00f4ng native EventSource/token URL;\n\n## Security and migration notes\n\nBounded buffer/backoff, no content logs.\n\n## Out of scope\n\nWebSocket.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 2 \u2014 Web SPA MVP" \
  --label "markhand-web,docs,web-p2,backlog"

create_if_missing "P2-05 \u2014 Login/session/application shell" \
  --title "P2-05 \u2014 Login/session/application shell" \
  --body "## Metadata\n\n- Milestone: Phase 2 \u2014 Web SPA MVP\n- Phase code: 2\n- Issue ID: P2-05\n- Status: `backlog`\n- Catalog: `backlog/phase-2/issues/README.md`\n- Phase plan: `phase-2-web-spa.md`\n\n## Dependencies / blocks\n\nP2-01/03 + P1B-F05 browser refresh contract.\nIntended route, expiry, guard matrix, login/refresh/logout component tests v\u00e0\nintegration CSRF/cookie-origin contract theo auth ADR.\n\n## Security and migration notes\n\nTransport theo auth ADR. N\u1ebfu ch\u1ecdn cookie: HttpOnly/Secure/SameSite +\nCSRF/Origin contract; n\u1ebfu ch\u1ecdn bearer refresh: kh\u00f4ng cookie/CSRF nh\u01b0ng token kh\u00f4ng\n\u0111\u01b0\u1ee3c persist/log. Server lu\u00f4n l\u00e0 authority. **Out:** signup/reset/MFA/OIDC.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 2 \u2014 Web SPA MVP" \
  --label "markhand-web,docs,web-p2,backlog"

create_if_missing "P2-06 \u2014 Org switch v\u00e0 scope-safe state" \
  --title "P2-06 \u2014 Org switch v\u00e0 scope-safe state" \
  --body "## Metadata\n\n- Milestone: Phase 2 \u2014 Web SPA MVP\n- Phase code: 2\n- Issue ID: P2-06\n- Status: `backlog`\n- Catalog: `backlog/phase-2/issues/README.md`\n- Phase plan: `phase-2-web-spa.md`\n\n## Dependencies / blocks\n\nP2-03\u202605 + backend 1C org APIs.\ndelayed/active-stream/rapid-switch/stale-membership tests.\n\n## Required tests / evidence\n\nNo old-org render;\n\n## Security and migration notes\n\nNo unapproved persisted tenant cache.\n\n## Out of scope\n\nsimultaneous org view.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 2 \u2014 Web SPA MVP" \
  --label "markhand-web,docs,web-p2,backlog"

create_if_missing "P2-07 \u2014 Library/list/sanitized preview" \
  --title "P2-07 \u2014 Library/list/sanitized preview" \
  --body "## Metadata\n\n- Milestone: Phase 2 \u2014 Web SPA MVP\n- Phase code: 2\n- Issue ID: P2-07\n- Status: `backlog`\n- Catalog: `backlog/phase-2/issues/README.md`\n- Phase plan: `phase-2-web-spa.md`\n\n## Dependencies / blocks\n\nP2-02/03/05/06.\npreview; unsafe markdown, 403/404, switch-race tests.\n\n## Required tests / evidence\n\nStable URL/pagination; API-only\n\n## Security and migration notes\n\nNo local path/public key.\n\n## Out of scope\n\ndesktop editor/compare.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 2 \u2014 Web SPA MVP" \
  --label "markhand-web,docs,web-p2,backlog"

create_if_missing "P2-08 \u2014 Upload progress v\u00e0 job lifecycle" \
  --title "P2-08 \u2014 Upload progress v\u00e0 job lifecycle" \
  --body "## Metadata\n\n- Milestone: Phase 2 \u2014 Web SPA MVP\n- Phase code: 2\n- Issue ID: P2-08\n- Status: `backlog`\n- Catalog: `backlog/phase-2/issues/README.md`\n- Phase plan: `phase-2-web-spa.md`\n\n## Dependencies / blocks\n\nP2-04/07.\nrefresh; success/cancel/loss/gap/413/415/429/filename tests.\n\n## Required tests / evidence\n\nClient/server progress distinct; recover\n\n## Security and migration notes\n\nNo client conversion queue.\n\n## Out of scope\n\nfolder/watch/resumable protocol.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 2 \u2014 Web SPA MVP" \
  --label "markhand-web,docs,web-p2,backlog"

create_if_missing "P2-09 \u2014 Download/delete/reindex/retry" \
  --title "P2-09 \u2014 Download/delete/reindex/retry" \
  --body "## Metadata\n\n- Milestone: Phase 2 \u2014 Web SPA MVP\n- Phase code: 2\n- Issue ID: P2-09\n- Status: `backlog`\n- Catalog: `backlog/phase-2/issues/README.md`\n- Phase plan: `phase-2-web-spa.md`\n\n## Dependencies / blocks\n\nP2-07/08 + backend 1C guards.\nserver deny wins; confirm/concurrency/stale/signed-route tests.\n\n## Required tests / evidence\n\nDelete closes preview;\n\n## Security and migration notes\n\nNo client-built object URLs; CSRF/idempotency.\n\n## Out of scope\n\npurge policy.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 2 \u2014 Web SPA MVP" \
  --label "markhand-web,docs,web-p2,backlog"

create_if_missing "P2-10 \u2014 Streaming search/Q&A/citations" \
  --title "P2-10 \u2014 Streaming search/Q&A/citations" \
  --body "## Metadata\n\n- Milestone: Phase 2 \u2014 Web SPA MVP\n- Phase code: 2\n- Issue ID: P2-10\n- Status: `backlog`\n- Catalog: `backlog/phase-2/issues/README.md`\n- Phase plan: `phase-2-web-spa.md`\n\n## Dependencies / blocks\n\nP2-04\u202607 + backend ACL.\ncitation; multi-document citations; old/new amount example labels v1/v2 and delta;\nBA 10m vs design 15m warning then v2 resolved; as-of/history/deep-link/sequence/\nfallback/no-answer/revoke/switch-mid-answer tests.\n\n## Required tests / evidence\n\n`aria-live`; current source\n\n## Security and migration notes\n\nSanitized Markdown/server route IDs.\n\n## Out of scope\n\nintelligence/conversation memory.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 2 \u2014 Web SPA MVP" \
  --label "markhand-web,docs,web-p2,backlog"

create_if_missing "P2-11 \u2014 Member/role admin" \
  --title "P2-11 \u2014 Member/role admin" \
  --body "## Metadata\n\n- Milestone: Phase 2 \u2014 Web SPA MVP\n- Phase code: 2\n- Issue ID: P2-11\n- Status: `backlog`\n- Catalog: `backlog/phase-2/issues/README.md`\n- Phase plan: `phase-2-web-spa.md`\n\n## Dependencies / blocks\n\nP2-02/03/05 + backend 1C-02\u202604.\nlast-owner conflict, invite/suspend/role/403/409/stale-update tests.\n\n## Required tests / evidence\n\nOwner/admin matrix,\n\n## Security and migration notes\n\nUI kh\u00f4ng hard-code matrix hay thay enforcement.\n\n## Out of scope\n\ncustom/group/SSO.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 2 \u2014 Web SPA MVP" \
  --label "markhand-web,docs,web-p2,backlog"

create_if_missing "P2-12 \u2014 Usage/quota/reservations" \
  --title "P2-12 \u2014 Usage/quota/reservations" \
  --body "## Metadata\n\n- Milestone: Phase 2 \u2014 Web SPA MVP\n- Phase code: 2\n- Issue ID: P2-12\n- Status: `backlog`\n- Catalog: `backlog/phase-2/issues/README.md`\n- Phase plan: `phase-2-web-spa.md`\n\n## Dependencies / blocks\n\nP2-03/05 + backend 1C-09\u202611.\nunit/timezone/403/429/stale tests.\n\n## Required tests / evidence\n\nAPI numbers match;\n\n## Security and migration notes\n\nNo client-derived authority/cross-org usage.\n\n## Out of scope\n\nbilling.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 2 \u2014 Web SPA MVP" \
  --label "markhand-web,docs,web-p2,backlog"

create_if_missing "P2-13 \u2014 Browser/SafeMarkdown hardening" \
  --title "P2-13 \u2014 Browser/SafeMarkdown hardening" \
  --body "## Metadata\n\n- Milestone: Phase 2 \u2014 Web SPA MVP\n- Phase code: 2\n- Issue ID: P2-13\n- Status: `backlog`\n- Catalog: `backlog/phase-2/issues/README.md`\n- Phase plan: `phase-2-web-spa.md`\n\n## Dependencies / blocks\n\nP2-01/07/10.\nbrowser/OWASP/dependency tests; no inline eval.\n\n## Required tests / evidence\n\nMalicious corpus kh\u00f4ng execute; CSP\n\n## Security and migration notes\n\nCSP/frame/nosniff/referrer/HSTS proxy.\n\n## Out of scope\n\nWAF/pentest.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 2 \u2014 Web SPA MVP" \
  --label "markhand-web,docs,web-p2,backlog"

create_if_missing "P2-14 \u2014 Accessibility/interaction quality" \
  --title "P2-14 \u2014 Accessibility/interaction quality" \
  --body "## Metadata\n\n- Milestone: Phase 2 \u2014 Web SPA MVP\n- Phase code: 2\n- Issue ID: P2-14\n- Status: `backlog`\n- Catalog: `backlog/phase-2/issues/README.md`\n- Phase plan: `phase-2-web-spa.md`\n\n## Dependencies / blocks\n\nP2-05/07\u202612.\nflows; focus/reduced-motion/screen reader tests.\n\n## Required tests / evidence\n\nNo axe critical; keyboard primary\n\n## Security and migration notes\n\nError kh\u00f4ng \u0111\u1ecdc internal/token.\n\n## Out of scope\n\nformal certification/i18n.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 2 \u2014 Web SPA MVP" \
  --label "markhand-web,docs,web-p2,backlog"

create_if_missing "P2-15 \u2014 Contract/integration/E2E suite" \
  --title "P2-15 \u2014 Contract/integration/E2E suite" \
  --body "## Metadata\n\n- Milestone: Phase 2 \u2014 Web SPA MVP\n- Phase code: 2\n- Issue ID: P2-15\n- Status: `backlog`\n- Catalog: `backlog/phase-2/issues/README.md`\n- Phase plan: `phase-2-web-spa.md`\n\n## Dependencies / blocks\n\nP2-02\u202614; deployed integration c\u1ea7n 1C endpoints.\n\n## Required tests / evidence\n\nMock deterministic + real deployment E2E; no stale-scope render;\ndesktop regression.\n\n## Security and migration notes\n\nEphemeral users/credentials.\n\n## Out of scope\n\nthay backend denial suite.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 2 \u2014 Web SPA MVP" \
  --label "markhand-web,docs,web-p2,backlog"

create_if_missing "P2-16 \u2014 Production build/static serving/final gate" \
  --title "P2-16 \u2014 Production build/static serving/final gate" \
  --body "## Metadata\n\n- Milestone: Phase 2 \u2014 Web SPA MVP\n- Phase code: 2\n- Issue ID: P2-16\n- Status: `backlog`\n- Catalog: `backlog/phase-2/issues/README.md`\n- Phase plan: `phase-2-web-spa.md`\n\n## Dependencies / blocks\n\nP2-13/15 + 1C-12/13.\n\n## Required tests / evidence\n\nDeep-link, cache/header/API404, packaged E2E, SLO, scans,\ndesktop test/build.\n\n## Security and migration notes\n\nMock/source map policy; rollbackable immutable assets.\n\n## Out of scope\n\nCDN/HA.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 2 \u2014 Web SPA MVP" \
  --label "markhand-web,docs,web-p2,backlog"

create_if_missing "P3-01 \u2014 Reusable intelligence service boundary" \
  --title "P3-01 \u2014 Reusable intelligence service boundary" \
  --body "## Metadata\n\n- Milestone: Phase 3 \u2014 Document Intelligence\n- Phase code: 3\n- Issue ID: P3-01\n- Status: `backlog`\n- Catalog: `backlog/phase-3/issues/README.md`\n- Phase plan: `phase-3-intelligence.md`\n\n## Dependencies / blocks\n\nPhase 2 + existing jobs/storage/audit.\ng\u1ecdi core tr\u1ef1c ti\u1ebfp; every call scoped; parity/adapter/missing-org/desktop tests.\n\n## Required tests / evidence\n\nKh\u00f4ng route\n\n## Security and migration notes\n\nNo document logs.\n\n## Out of scope\n\nwatch folder/rewrite core algorithms.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 3 \u2014 Document Intelligence" \
  --label "markhand-web,docs,web-p3,backlog"

create_if_missing "P3-02 \u2014 Versioned derived-artifact schema" \
  --title "P3-02 \u2014 Versioned derived-artifact schema" \
  --body "## Metadata\n\n- Milestone: Phase 3 \u2014 Document Intelligence\n- Phase code: 3\n- Issue ID: P3-02\n- Status: `backlog`\n- Catalog: `backlog/phase-3/issues/README.md`\n- Phase plan: `phase-3-intelligence.md`\n\n## Dependencies / blocks\n\nP3-01 + ACL model.\nduplicate; fresh/upgrade/tenancy/reconcile/idempotency/conflict tests.\n\n## Required tests / evidence\n\nFull provenance; retry no\n\n## Security and migration notes\n\nExpand/backfill/cutover, no object key; ACL snapshot tuy\u1ec7t\n\u0111\u1ed1i kh\u00f4ng d\u00f9ng \u0111\u1ec3 authorize. **Out:** mutable/shared artifact.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 3 \u2014 Document Intelligence" \
  --label "markhand-web,docs,web-p3,backlog"

create_if_missing "P3-03 \u2014 Current-source ACL m\u1ed7i artifact access" \
  --title "P3-03 \u2014 Current-source ACL m\u1ed7i artifact access" \
  --body "## Metadata\n\n- Milestone: Phase 3 \u2014 Document Intelligence\n- Phase code: 3\n- Issue ID: P3-03\n- Status: `backlog`\n- Catalog: `backlog/phase-3/issues/README.md`\n- Phase plan: `phase-3-intelligence.md`\n\n## Dependencies / blocks\n\nP3-02 + 1C denial suite.\nngay; cross-scope/revoke-race/cache/signed-url/existence tests.\n\n## Required tests / evidence\n\nRevoke b\u1ea5t k\u1ef3 source deny\n\n## Security and migration notes\n\nTimeout fail closed; snapshot kh\u00f4ng authorize.\n\n## Out of scope\n\npublic links.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 3 \u2014 Document Intelligence" \
  --label "markhand-web,docs,web-p3,backlog"

create_if_missing "P3-04 \u2014 Deterministic BRD/PRD job" \
  --title "P3-04 \u2014 Deterministic BRD/PRD job" \
  --body "## Metadata\n\n- Milestone: Phase 3 \u2014 Document Intelligence\n- Phase code: 3\n- Issue ID: P3-04\n- Status: `backlog`\n- Catalog: `backlog/phase-3/issues/README.md`\n- Phase plan: `phase-3-intelligence.md`\n\n## Dependencies / blocks\n\nP3-01\u202603.\ncitation; kh\u00f4ng evidence\u2192question; offline/empty/kill/retry/NFC tests.\n\n## Required tests / evidence\n\n\u0110\u1ee7 10 artifacts; factual requirement c\u00f3\n\n## Security and migration notes\n\nVersioned payload/schema, content-safe audit.\n\n## Out of scope\n\nthird-party publish.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 3 \u2014 Document Intelligence" \
  --label "markhand-web,docs,web-p3,backlog"

create_if_missing "P3-05 \u2014 Handoff edit/validate/ZIP export" \
  --title "P3-05 \u2014 Handoff edit/validate/ZIP export" \
  --body "## Metadata\n\n- Milestone: Phase 3 \u2014 Document Intelligence\n- Phase code: 3\n- Issue ID: P3-05\n- Status: `backlog`\n- Catalog: `backlog/phase-3/issues/README.md`\n- Phase plan: `phase-3-intelligence.md`\n\n## Dependencies / blocks\n\nP3-03/04.\nconcurrent/revoke/large export tests.\n\n## Required tests / evidence\n\nNo overwrite; manifest round-trip/tamper/\n\n## Security and migration notes\n\nSafe archive names, no public URL.\n\n## Out of scope\n\ndirect Jira/Confluence import.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 3 \u2014 Document Intelligence" \
  --label "markhand-web,docs,web-p3,backlog"

create_if_missing "P3-06 \u2014 Quality v\u00e0 immutable reprocess" \
  --title "P3-06 \u2014 Quality v\u00e0 immutable reprocess" \
  --body "## Metadata\n\n- Milestone: Phase 3 \u2014 Document Intelligence\n- Phase code: 3\n- Issue ID: P3-06\n- Status: `backlog`\n- Catalog: `backlog/phase-3/issues/README.md`\n- Phase plan: `phase-3-intelligence.md`\n\n## Dependencies / blocks\n\nP3-01\u202603 + converter.\nno cloud; OCR/VLM/fallback/quota/cancel/retry tests.\n\n## Required tests / evidence\n\nSource immutable; policy deny\n\n## Security and migration notes\n\nVLM deny-by-default.\n\n## Out of scope\n\nfake unsupported success.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 3 \u2014 Document Intelligence" \
  --label "markhand-web,docs,web-p3,backlog"

create_if_missing "P3-07 \u2014 Citation-backed summarization" \
  --title "P3-07 \u2014 Citation-backed summarization" \
  --body "## Metadata\n\n- Milestone: Phase 3 \u2014 Document Intelligence\n- Phase code: 3\n- Issue ID: P3-07\n- Status: `backlog`\n- Catalog: `backlog/phase-3/issues/README.md`\n- Phase plan: `phase-3-intelligence.md`\n\n## Dependencies / blocks\n\nP3-01\u202603 + retrieval.\nfallback/insufficient/timeout/revoke/factuality golden tests.\n\n## Required tests / evidence\n\nEvery fact attributable;\n\n## Security and migration notes\n\nProvider metadata only, no prompt logs.\n\n## Out of scope\n\nweb research.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 3 \u2014 Document Intelligence" \
  --label "markhand-web,docs,web-p3,backlog"

create_if_missing "P3-08 \u2014 PII detection/reviewed redaction" \
  --title "P3-08 \u2014 PII detection/reviewed redaction" \
  --body "## Metadata\n\n- Milestone: Phase 3 \u2014 Document Intelligence\n- Phase code: 3\n- Issue ID: P3-08\n- Status: `backlog`\n- Catalog: `backlog/phase-3/issues/README.md`\n- Phase plan: `phase-3-intelligence.md`\n\n## Dependencies / blocks\n\nP3-02/03 + RBAC.\nauthorized; precision/recall/completeness/overlap/Unicode/permission tests.\n\n## Required tests / evidence\n\nOriginal hash unchanged; findings\n\n## Security and migration notes\n\nNo PII logs; prohibited data no GLM.\n\n## Out of scope\n\nlegal completeness guarantee.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 3 \u2014 Document Intelligence" \
  --label "markhand-web,docs,web-p3,backlog"

create_if_missing "P3-09 \u2014 Schema/table edit v\u00e0 safe CSV" \
  --title "P3-09 \u2014 Schema/table edit v\u00e0 safe CSV" \
  --body "## Metadata\n\n- Milestone: Phase 3 \u2014 Document Intelligence\n- Phase code: 3\n- Issue ID: P3-09\n- Status: `backlog`\n- Catalog: `backlog/phase-3/issues/README.md`\n- Phase plan: `phase-3-intelligence.md`\n\n## Dependencies / blocks\n\nP3-02/03.\nmultiline/stable ID/formula/ACL tests.\n\n## Required tests / evidence\n\nByte-preserving round trip; escaped pipes/\n\n## Security and migration notes\n\nSource hash binding.\n\n## Out of scope\n\nspreadsheet formulas/merged edit/realtime.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 3 \u2014 Document Intelligence" \
  --label "markhand-web,docs,web-p3,backlog"

create_if_missing "P3-10 \u2014 Versions, diff v\u00e0 three-way merge" \
  --title "P3-10 \u2014 Versions, diff v\u00e0 three-way merge" \
  --body "## Metadata\n\n- Milestone: Phase 3 \u2014 Document Intelligence\n- Phase code: 3\n- Issue ID: P3-10\n- Status: `backlog`\n- Catalog: `backlog/phase-3/issues/README.md`\n- Phase plan: `phase-3-intelligence.md`\n\n## Dependencies / blocks\n\nP3-02/03.\nstale overwrite; Unicode/CRLF/concurrent/revoked-base tests.\n\n## Required tests / evidence\n\nUnrelated merge, exact conflicts, no\n\n## Security and migration notes\n\nImmutable rows/opaque keys.\n\n## Out of scope\n\ncollaborative editing.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 3 \u2014 Document Intelligence" \
  --label "markhand-web,docs,web-p3,backlog"

create_if_missing "P3-11 \u2014 Prompt/model/quota/cloud policy" \
  --title "P3-11 \u2014 Prompt/model/quota/cloud policy" \
  --body "## Metadata\n\n- Milestone: Phase 3 \u2014 Document Intelligence\n- Phase code: 3\n- Issue ID: P3-11\n- Status: `backlog`\n- Catalog: `backlog/phase-3/issues/README.md`\n- Phase plan: `phase-3-intelligence.md`\n\n## Dependencies / blocks\n\nP3-01 + quota/classification.\ntoken race/provider failure/template rollback tests.\n\n## Required tests / evidence\n\nInjection/egress deny/\n\n## Security and migration notes\n\nSecret manager, metadata-only audit.\n\n## Out of scope\n\ngeneral agents/tools.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 3 \u2014 Document Intelligence" \
  --label "markhand-web,docs,web-p3,backlog"

create_if_missing "P3-12 \u2014 Intelligence web workspace" \
  --title "P3-12 \u2014 Intelligence web workspace" \
  --body "## Metadata\n\n- Milestone: Phase 3 \u2014 Document Intelligence\n- Phase code: 3\n- Issue ID: P3-12\n- Status: `backlog`\n- Catalog: `backlog/phase-3/issues/README.md`\n- Phase plan: `phase-3-intelligence.md`\n\n## Dependencies / blocks\n\nP3-04\u202610 APIs.\nartifact state; component/a11y/Playwright flows.\n\n## Required tests / evidence\n\nIndependent panels; no stale PII/\n\n## Security and migration notes\n\nNo storage-key URL.\n\n## Out of scope\n\nQ&A panel/watch/native dialogs.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 3 \u2014 Document Intelligence" \
  --label "markhand-web,docs,web-p3,backlog"

create_if_missing "P3-13 \u2014 Task-specific golden evaluation" \
  --title "P3-13 \u2014 Task-specific golden evaluation" \
  --body "## Metadata\n\n- Milestone: Phase 3 \u2014 Document Intelligence\n- Phase code: 3\n- Issue ID: P3-13\n- Status: `backlog`\n- Catalog: `backlog/phase-3/issues/README.md`\n- Phase plan: `phase-3-intelligence.md`\n\n## Dependencies / blocks\n\nP3-04\u202611.\nbudgets; reproducible case-level failures; retrieval score kh\u00f4ng thay task metrics.\n\n## Required tests / evidence\n\nIndependent numeric thresholds/regression\n\n## Security and migration notes\n\nLicensed/de-identified corpus.\n\n## Out of scope\n\nblended \u201cAI quality\u201d score.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 3 \u2014 Document Intelligence" \
  --label "markhand-web,docs,web-p3,backlog"

create_if_missing "P3-14 \u2014 Intelligence denial/audit/reconcile/release gate" \
  --title "P3-14 \u2014 Intelligence denial/audit/reconcile/release gate" \
  --body "## Metadata\n\n- Milestone: Phase 3 \u2014 Document Intelligence\n- Phase code: 3\n- Issue ID: P3-14\n- Status: `backlog`\n- Catalog: `backlog/phase-3/issues/README.md`\n- Phase plan: `phase-3-intelligence.md`\n\n## Dependencies / blocks\n\nP3-03\u202613.\nno canonical mutation; provider outage; desktop regression.\n\n## Required tests / evidence\n\nZero leakage; current ACL/revoke races;\n\n## Security and migration notes\n\nReconcile kh\u00f4ng restore revoked visibility.\n\n## Out of scope\n\nOIDC/production infra.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 3 \u2014 Document Intelligence" \
  --label "markhand-web,docs,web-p3,backlog"

create_if_missing "P4-01 \u2014 OIDC Authorization Code + PKCE" \
  --title "P4-01 \u2014 OIDC Authorization Code + PKCE" \
  --body "## Metadata\n\n- Milestone: Phase 4 \u2014 Production Hardening\n- Phase code: 4\n- Issue ID: P4-01\n- Status: `backlog`\n- Catalog: `backlog/phase-4/issues/README.md`\n- Phase plan: `phase-4-production-hardening.md`\n\n## Dependencies / blocks\n\nExisting provider/session framework.\n`(issuer,subject)`, never email; invalid/replay/rotated-key/multi-issuer mock+staging.\n\n## Required tests / evidence\n\nIdentity\n\n## Security and migration notes\n\nNo code/token logs, separate identity linkage.\n\n## Out of scope\n\nSAML.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 4 \u2014 Production Hardening" \
  --label "markhand-web,docs,web-p4,backlog"

create_if_missing "P4-02 \u2014 JIT provisioning v\u00e0 account linking" \
  --title "P4-02 \u2014 JIT provisioning v\u00e0 account linking" \
  --body "## Metadata\n\n- Milestone: Phase 4 \u2014 Production Hardening\n- Phase code: 4\n- Issue ID: P4-02\n- Status: `backlog`\n- Catalog: `backlog/phase-4/issues/README.md`\n- Phase plan: `phase-4-production-hardening.md`\n\n## Dependencies / blocks\n\nP4-01.\nkh\u00f4ng transfer; spoof/duplicate/disabled-JIT/link/unlink tests.\n\n## Required tests / evidence\n\nAmbiguous mapping fail closed; email change\n\n## Security and migration notes\n\nUnique immutable issuer+subject audit.\n\n## Out of scope\n\nuser policy DSL.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 4 \u2014 Production Hardening" \
  --label "markhand-web,docs,web-p4,backlog"

create_if_missing "P4-03 \u2014 Group/role sync v\u00e0 bounded deprovision" \
  --title "P4-03 \u2014 Group/role sync v\u00e0 bounded deprovision" \
  --body "## Metadata\n\n- Milestone: Phase 4 \u2014 Production Hardening\n- Phase code: 4\n- Issue ID: P4-03\n- Status: `backlog`\n- Catalog: `backlog/phase-4/issues/README.md`\n- Phase plan: `phase-4-production-hardening.md`\n\n## Dependencies / blocks\n\nP4-01/02 + IdP capability.\nk\u1ec3 c\u1ea3 missed sync/outage; replay/order/group removal/in-flight tests.\n\n## Required tests / evidence\n\nMeasured revoke bound\n\n## Security and migration notes\n\nSigned webhook/least SCIM token/audit.\n\n## Out of scope\n\ndocument-driven identity.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 4 \u2014 Production Hardening" \
  --label "markhand-web,docs,web-p4,backlog"

create_if_missing "P4-04 \u2014 Session UI v\u00e0 break-glass" \
  --title "P4-04 \u2014 Session UI v\u00e0 break-glass" \
  --body "## Metadata\n\n- Milestone: Phase 4 \u2014 Production Hardening\n- Phase code: 4\n- Issue ID: P4-04\n- Status: `backlog`\n- Catalog: `backlog/phase-4/issues/README.md`\n- Phase plan: `phase-4-production-hardening.md`\n\n## Dependencies / blocks\n\nP4-01\u202603.\nbreak-glass/rotation/race drill.\n\n## Required tests / evidence\n\nRevoked REST/SSE denied; IdP outage\n\n## Security and migration notes\n\nEmergency use always high-signal audit.\n\n## Out of scope\n\nsocial login.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 4 \u2014 Production Hardening" \
  --label "markhand-web,docs,web-p4,backlog"

create_if_missing "P4-05 \u2014 Platform security/supply chain/secrets" \
  --title "P4-05 \u2014 Platform security/supply chain/secrets" \
  --body "## Metadata\n\n- Milestone: Phase 4 \u2014 Production Hardening\n- Phase code: 4\n- Issue ID: P4-05\n- Status: `backlog`\n- Catalog: `backlog/phase-4/issues/README.md`\n- Phase plan: `phase-4-production-hardening.md`\n\n## Dependencies / blocks\n\nDeployment target; c\u00f3 th\u1ec3 song song OIDC.\nsecret; compromise drill; policy-enforced scans/security probes/rotation.\n\n## Required tests / evidence\n\nNo static\n\n## Security and migration notes\n\nStaged dual-key rotation.\n\n## Out of scope\n\ncustom crypto.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 4 \u2014 Production Hardening" \
  --label "markhand-web,docs,web-p4,backlog"

create_if_missing "P4-06 \u2014 Threat review/external pentest/remediation" \
  --title "P4-06 \u2014 Threat review/external pentest/remediation" \
  --body "## Metadata\n\n- Milestone: Phase 4 \u2014 Production Hardening\n- Phase code: 4\n- Issue ID: P4-06\n- Status: `backlog`\n- Catalog: `backlog/phase-4/issues/README.md`\n- Phase plan: `phase-4-production-hardening.md`\n\n## Dependencies / blocks\n\nP4-01\u202605 + stable staging.\nhigh/critical; risk \u0111\u01b0\u1ee3c formally accepted ph\u1ea3i c\u00f3 approver, compensating controls,\nexpiry v\u00e0 retest date; external retest evidence.\n\n## Required tests / evidence\n\nZero unresolved\n\n## Security and migration notes\n\nRestricted report handling.\n\n## Out of scope\n\nself-review thay pentest.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 4 \u2014 Production Hardening" \
  --label "markhand-web,docs,web-p4,backlog"

create_if_missing "P4-07 \u2014 HA/degraded modes/distributed limiting" \
  --title "P4-07 \u2014 HA/degraded modes/distributed limiting" \
  --body "## Metadata\n\n- Milestone: Phase 4 \u2014 Production Hardening\n- Phase code: 4\n- Issue ID: P4-07\n- Status: `backlog`\n- Catalog: `backlog/phase-4/issues/README.md`\n- Phase plan: `phase-4-production-hardening.md`\n\n## Dependencies / blocks\n\nSLA/deployment ADR.\nreplica restart; authorized degraded behavior; fault/failover/load tests.\n\n## Required tests / evidence\n\nAggregate limits survive\n\n## Security and migration notes\n\nAuth/storage failure fail closed.\n\n## Out of scope\n\nmulti-site active-active.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 4 \u2014 Production Hardening" \
  --label "markhand-web,docs,web-p4,backlog"

create_if_missing "P4-08 \u2014 Reproducible production deployment" \
  --title "P4-08 \u2014 Reproducible production deployment" \
  --body "## Metadata\n\n- Milestone: Phase 4 \u2014 Production Hardening\n- Phase code: 4\n- Issue ID: P4-08\n- Status: `backlog`\n- Catalog: `backlog/phase-4/issues/README.md`\n- Phase plan: `phase-4-production-hardening.md`\n\n## Dependencies / blocks\n\nP4-05/07.\npolicy/install/upgrade/rollback/uninstall/pressure tests.\n\n## Required tests / evidence\n\nClean install; clear POC-vs-HA; lint/\n\n## Security and migration notes\n\nLeast service accounts, no public data services.\n\n## Out of scope\n\nCompose as prod.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 4 \u2014 Production Hardening" \
  --label "markhand-web,docs,web-p4,backlog"

create_if_missing "P4-09 \u2014 Backup/PITR/restore tooling" \
  --title "P4-09 \u2014 Backup/PITR/restore tooling" \
  --body "## Metadata\n\n- Milestone: Phase 4 \u2014 Production Hardening\n- Phase code: 4\n- Issue ID: P4-09\n- Status: `backlog`\n- Catalog: `backlog/phase-4/issues/README.md`\n- Phase plan: `phase-4-production-hardening.md`\n\n## Dependencies / blocks\n\nP4-08 + RPO/RTO.\nincompatible before ready; scheduled restore/checksum/key tests.\n\n## Required tests / evidence\n\nDetect missing/orphan/corrupt/\n\n## Security and migration notes\n\nEncrypt/isolate/audit backup credentials.\n\n## Out of scope\n\nQdrant-only backup.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 4 \u2014 Production Hardening" \
  --label "markhand-web,docs,web-p4,backlog"

create_if_missing "P4-10 \u2014 Clean DR/destructive failure drills" \
  --title "P4-10 \u2014 Clean DR/destructive failure drills" \
  --body "## Metadata\n\n- Milestone: Phase 4 \u2014 Production Hardening\n- Phase code: 4\n- Issue ID: P4-10\n- Status: `backlog`\n- Catalog: `backlog/phase-4/issues/README.md`\n- Phase plan: `phase-4-production-hardening.md`\n\n## Dependencies / blocks\n\nP4-08/09.\nscenario evidence.\n\n## Required tests / evidence\n\nApproved RPO/RTO on clean environment,\n\n## Security and migration notes\n\nIsolated restored staging; rotate compromised keys.\n\n## Out of scope\n\npaper drill.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 4 \u2014 Production Hardening" \
  --label "markhand-web,docs,web-p4,backlog"

create_if_missing "P4-11 \u2014 Migration/canary/rollback discipline" \
  --title "P4-11 \u2014 Migration/canary/rollback discipline" \
  --body "## Metadata\n\n- Milestone: Phase 4 \u2014 Production Hardening\n- Phase code: 4\n- Issue ID: P4-11\n- Status: `backlog`\n- Catalog: `backlog/phase-4/issues/README.md`\n- Phase plan: `phase-4-production-hardening.md`\n\n## Dependencies / blocks\n\nP4-08 + schema features.\nsupported release; mixed workers; long backfill; shadow quality; trigger simulation.\n\n## Required tests / evidence\n\nUpgrade/rollback every\n\n## Security and migration notes\n\nIsolation incident immediate stop/rollback.\n\n## Out of scope\n\ndestructive down migration.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 4 \u2014 Production Hardening" \
  --label "markhand-web,docs,web-p4,backlog"

create_if_missing "P4-12 \u2014 SRE dashboards/alerts/runbooks" \
  --title "P4-12 \u2014 SRE dashboards/alerts/runbooks" \
  --body "## Metadata\n\n- Milestone: Phase 4 \u2014 Production Hardening\n- Phase code: 4\n- Issue ID: P4-12\n- Status: `backlog`\n- Catalog: `backlog/phase-4/issues/README.md`\n- Phase plan: `phase-4-production-hardening.md`\n\n## Dependencies / blocks\n\nP4-07\u202611.\njobs/parser/outage/quota/credential/tenant/disk game days.\n\n## Required tests / evidence\n\nControlled alerts; operators resolve\n\n## Security and migration notes\n\nNo content/prompt/token/URL/PII telemetry.\n\n## Out of scope\n\nstaffing policy.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 4 \u2014 Production Hardening" \
  --label "markhand-web,docs,web-p4,backlog"

create_if_missing "P4-13 \u2014 Onboarding/help/accessibility/operator docs" \
  --title "P4-13 \u2014 Onboarding/help/accessibility/operator docs" \
  --body "## Metadata\n\n- Milestone: Phase 4 \u2014 Production Hardening\n- Phase code: 4\n- Issue ID: P4-13\n- Status: `backlog`\n- Catalog: `backlog/phase-4/issues/README.md`\n- Phase plan: `phase-4-production-hardening.md`\n\n## Dependencies / blocks\n\nStable P4 UI/APIs.\nWCAG 2.1 AA critical flows; usability/axe/keyboard/screen-reader/docs dry run.\n\n## Required tests / evidence\n\nNew users finish unaided;\n\n## Security and migration notes\n\nAccurate privacy/egress/RBAC/quota guidance.\n\n## Out of scope\n\nbilling docs.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 4 \u2014 Production Hardening" \
  --label "markhand-web,docs,web-p4,backlog"

create_if_missing "P4-14 \u2014 Production go-live gate" \
  --title "P4-14 \u2014 Production go-live gate" \
  --body "## Metadata\n\n- Milestone: Phase 4 \u2014 Production Hardening\n- Phase code: 4\n- Issue ID: P4-14\n- Status: `backlog`\n- Catalog: `backlog/phase-4/issues/README.md`\n- Phase plan: `phase-4-production-hardening.md`\n\n## Dependencies / blocks\n\nP4-03\u202613.\nunresolved high/critical (accepted risk ph\u1ea3i \u0111\u1ee7 approver/control/expiry/retest);\ndeprovision/RPO/RTO/HA/rollback/operator/onboarding gates all pass.\n\n## Required tests / evidence\n\nMandatory failures block release; zero\n\n## Security and migration notes\n\nIsolation incident lu\u00f4n hard blocker.\n\n## Out of scope\n\npost-P4 roadmap.\n\n## Source\n\nGenerated from Markhand Web backlog catalog. Update the catalog first, then re-run `python3 scripts/sync-github-issues.py --update` if specs change.\n" \
  --milestone "Phase 4 \u2014 Production Hardening" \
  --label "markhand-web,docs,web-p4,backlog"

