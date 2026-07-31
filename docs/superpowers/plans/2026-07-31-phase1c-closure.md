# Phase 1C Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close Phase 1C with evidence-backed RBAC/ACL enforcement, a connected multi-org denial suite, and a qualifying deployed security/load gate.

**Architecture:** Deliver five sequential PRs from fresh `master` branches. Establish one canonical RBAC contract, then make PostgreSQL/OrgContext ACL semantics equivalent, add machine-checked route/service guards and least-privilege identities, connect all denial evidence through one manifest, and finally qualify the multi-org POC through `G1C-*` gates. Each PR uses RED → GREEN, an independent Grok/Composer review, GitHub CI evidence, and conservative status synchronization.

**Tech Stack:** Rust 1.88, axum, tokio-postgres/PostgreSQL 16, Qdrant, MinIO, JSON-compatible YAML, Python 3.12, React 19/TypeScript/Vite, pnpm 10.33.3, Docker Compose v2, GitHub Actions.

## Global Constraints

- Design source: `docs/superpowers/specs/2026-07-31-phase1c-closure-design.md`.
- `vendor/markitdown-rs/` remains reference-only and must not become a dependency.
- Every tenant read and mutation fails closed; no `internal=true` authorization bypass.
- `OrgContext` continues to derive current membership and permissions from PostgreSQL, not client claims.
- Historical migrations are immutable. New schema work uses `0036_expand_acl_groups_invariants.sql`, updates `crates/server/migrations/manifest.json`, and remains expand-only.
- DB/service-backed tests stay `#[ignore]`; GitHub `rust-integration` runs them with `--include-ignored`.
- `MARKHAND_TEST_REQUIRED=1` forbids prerequisite soft-skips in CI/gate mode while preserving local ignored-test skips when unset.
- No fake `export`, autocomplete, PII, intelligence, or settings endpoint is created to satisfy a checklist.
- Reserved permission keys remain absent from runtime `permissions` and ungranted until a real operation activates them.
- Qualifying embedding is local/mock only until embedding-token metering exists.
- `AR-1C-AUDIT-RETENTION` is accepted only for POC/non-production and expires before production multi-org or the Phase 4 gate.
- Reports contain no password, token, capability URL, document text, prompt, PII, absolute workspace path, or secret-bearing DSN.
- Before every Rust PR push run:

```bash
cargo fmt --all -- --check
cargo metadata --locked --format-version 1 --no-deps
python3 scripts/check-dependency-policy.py
```

- A dependency manifest edit must include `Cargo.lock`.
- A status changes to `Done` only after the named CI/deployed evidence exists; path-filter skips and soft passes are not evidence.
- Each logical task is committed separately. Push the RED-test commit before running RED, push the implementation commit before running GREEN, and update the draft PR after each correction.

## Branch, PR, and Review Sequence

| PR | Branch | Implementer | Reviewer | Must merge before |
|---|---|---|---|---|
| 1 | `cursor/phase1c-rbac-foundation-6ddb` | Grok | Composer | PR 2 |
| 2 | `cursor/phase1c-acl-enforcement-6ddb` | Composer | Grok | PR 3 |
| 3 | `cursor/phase1c-guard-identities-6ddb` | Grok | Composer | PR 4 |
| 4 | `cursor/phase1c-denial-suite-6ddb` | Composer | Grok | PR 5 |
| 5 | `cursor/phase1c-security-gate-6ddb` | Grok | Composer | Phase closure |

For every task, the reviewer reads the actual diff and relevant source, runs the available test layer, and returns `APPROVED`, `APPROVED_WITH_CONCERNS`, or `CHANGES_REQUIRED`. Critical/Important findings return to the implementer and are reviewed again before the next task.

---

## PR 1 — RBAC Foundation and Catalog Truth

### Task 1: Add the canonical built-in role catalog

**Files:**
- Create: `crates/server/openapi/builtin-role-catalog.json`
- Create: `crates/server/src/auth/rbac_catalog.rs`
- Modify: `crates/server/src/auth/mod.rs`
- Test: unit tests inside `crates/server/src/auth/rbac_catalog.rs`

**Interfaces:**
- Produces: `BuiltinRoleCatalog`, `PermissionStatus`, `BuiltinPermission`, `load_builtin_role_catalog()`, `validate_catalog_invariants()`.
- Consumed later by: OpenAPI tests, web role presentation, DB matrix tests, PR 3 guard inventory.

- [ ] **Step 1: Write failing catalog loader and invariant tests**

Add these public types and tests before creating the JSON fixture:

```rust
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuiltinRoleCatalog {
    pub version: u32,
    pub roles: Vec<String>,
    pub permissions: Vec<BuiltinPermission>,
    pub grants: std::collections::BTreeMap<String, Vec<String>>,
    pub restrictions: Vec<RoleRestriction>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleRestriction {
    pub id: String,
    pub description: String,
    pub enforced_by: String,
}

#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionStatus {
    Active,
    Reserved,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuiltinPermission {
    pub key: String,
    pub status: PermissionStatus,
    pub description: String,
    pub required_collection_access: Option<crate::db::models::AccessLevel>,
    pub conditional_policy: Option<String>,
    pub operation_refs: Vec<String>,
}

pub fn load_builtin_role_catalog() -> BuiltinRoleCatalog {
    serde_json::from_str(include_str!("../../openapi/builtin-role-catalog.json"))
        .expect("builtin role catalog must be valid JSON")
}
```

Tests must assert:

```text
roles == owner, admin, editor, viewer
active keys == current nine runtime permissions
reserved keys == settings.manage, intel.use, pii.manage, export.run
every grant references an active key
no reserved key is granted
doc.quarantine.review has zero default grants
editor has no doc.delete
every active key has at least one operationRefs entry
every reserved key has no operationRefs entry
```

- [ ] **Step 2: Commit and push the RED test**

```bash
git checkout master
git pull origin master
git checkout -b cursor/phase1c-rbac-foundation-6ddb
git add crates/server/src/auth/rbac_catalog.rs crates/server/src/auth/mod.rs
git commit -m "test(server): define built-in RBAC catalog contract"
git push -u origin cursor/phase1c-rbac-foundation-6ddb
```

Create the draft PR against `master`, then run:

```bash
cargo test -p fileconv-server auth::rbac_catalog --lib
```

Expected: compile or `include_str!` failure because the canonical fixture does not exist.

- [ ] **Step 3: Add the complete fixture and validator**

Create version 1 with these normative grants:

```json
{
  "version": 1,
  "roles": ["owner", "admin", "editor", "viewer"],
  "permissions": [
    {"key":"doc.upload","status":"active","description":"Upload documents","requiredCollectionAccess":"write","conditionalPolicy":null,"operationRefs":["src/routes/uploads.rs"]},
    {"key":"doc.delete","status":"active","description":"Delete documents","requiredCollectionAccess":"admin","conditionalPolicy":"own_or_explicit_deferred_to_custom_role","operationRefs":["src/routes/documents.rs","src/routes/collections.rs"]},
    {"key":"doc.publish","status":"active","description":"Publish document versions","requiredCollectionAccess":"write","conditionalPolicy":null,"operationRefs":["src/routes/documents.rs"]},
    {"key":"qa.query","status":"active","description":"Query collections","requiredCollectionAccess":"read","conditionalPolicy":null,"operationRefs":["src/services/retrieval/mod.rs"]},
    {"key":"qa.history","status":"active","description":"Read version history","requiredCollectionAccess":"read","conditionalPolicy":null,"operationRefs":["src/services/access.rs"]},
    {"key":"member.manage","status":"active","description":"Manage members","requiredCollectionAccess":null,"conditionalPolicy":"admin_cannot_manage_owner","operationRefs":["src/routes/members.rs"]},
    {"key":"audit.view","status":"active","description":"Read audit entries","requiredCollectionAccess":null,"conditionalPolicy":null,"operationRefs":["src/routes/audit.rs"]},
    {"key":"jobs.system","status":"active","description":"Operate system jobs","requiredCollectionAccess":null,"conditionalPolicy":null,"operationRefs":["src/services/access.rs"]},
    {"key":"doc.quarantine.review","status":"active","description":"Approve quarantined uploads","requiredCollectionAccess":"write","conditionalPolicy":null,"operationRefs":["src/services/upload/saga.rs"]},
    {"key":"settings.manage","status":"reserved","description":"Reserved for organization settings","requiredCollectionAccess":null,"conditionalPolicy":"admin_except_owner_security_deferred","operationRefs":[]},
    {"key":"intel.use","status":"reserved","description":"Reserved for intelligence operations","requiredCollectionAccess":null,"conditionalPolicy":null,"operationRefs":[]},
    {"key":"pii.manage","status":"reserved","description":"Reserved for PII operations","requiredCollectionAccess":null,"conditionalPolicy":null,"operationRefs":[]},
    {"key":"export.run","status":"reserved","description":"Reserved for export operations","requiredCollectionAccess":null,"conditionalPolicy":"viewer_by_org_policy_deferred","operationRefs":[]}
  ],
  "grants": {
    "owner": ["audit.view","doc.delete","doc.publish","doc.upload","jobs.system","member.manage","qa.history","qa.query"],
    "admin": ["audit.view","doc.delete","doc.publish","doc.upload","jobs.system","member.manage","qa.history","qa.query"],
    "editor": ["doc.publish","doc.upload","qa.query"],
    "viewer": ["qa.query"]
  },
  "restrictions": [
    {"id":"admin_cannot_manage_owner","description":"Only an active owner may grant or manage the owner role","enforcedBy":"services::members::guard_owner_tier"}
  ]
}
```

`validate_catalog_invariants()` returns `Result<(), Vec<String>>`, sorts errors, and rejects duplicate roles, duplicate permission keys, unknown grants, reserved grants, missing operation references, active references to missing source files, and reserved references. For every active key, at least one referenced file must contain the exact quoted permission literal; file existence alone is not sufficient evidence. Task 9 adds `src/bin/worker.rs` to the `jobs.system` references only after the worker binary contains that literal.

- [ ] **Step 4: Commit, push, and verify GREEN**

```bash
git add crates/server/openapi/builtin-role-catalog.json \
  crates/server/src/auth/rbac_catalog.rs crates/server/src/auth/mod.rs
git commit -m "feat(server): add canonical built-in RBAC catalog"
git push -u origin cursor/phase1c-rbac-foundation-6ddb
cargo test -p fileconv-server auth::rbac_catalog --lib
```

Expected: all catalog tests pass.

- [ ] **Step 5: Independent review**

Composer reviews the Task 1 commit range for matrix accuracy, reserved-key behavior, and absence of a second grant matrix. Resolve all Important findings before Task 2.

---

### Task 2: Connect OpenAPI, web, and PostgreSQL to the catalog

**Files:**
- Modify: `crates/server/openapi/openapi.yaml`
- Modify: `crates/server/openapi/README.md`
- Modify: `crates/server/src/api/openapi.rs`
- Modify: `crates/server/tests/role_catalog.rs`
- Create: `web/src/rbac/builtinRoleCatalog.ts`
- Create: `web/src/rbac/builtinRoleCatalog.test.ts`
- Modify: `web/src/components/admin/memberPresentation.ts`
- Modify: `web/tsconfig.json`

**Interfaces:**
- OpenAPI exposes only `x-markhand-builtin-role-catalog: ./builtin-role-catalog.json`.
- Web derives role ordering from the JSON; presentation labels remain local UI copy.
- DB tests require runtime permission rows to equal active fixture keys and role grants to equal fixture grants.

- [ ] **Step 1: Add failing parity tests**

Add Rust tests:

```text
openapi_role_enums_match_builtin_catalog_roles
openapi_references_builtin_catalog_without_embedding_grants
canonical_matrix_matches_builtin_role_catalog_fixture
permissions_table_contains_exactly_active_catalog_keys
```

Add Vitest assertions:

```typescript
import { describe, expect, it } from 'vitest'
import { BUILTIN_ROLE_CATALOG, ROLE_ORDER } from './builtinRoleCatalog'

describe('builtin role catalog', () => {
  it('drives role ordering from the canonical fixture', () => {
    expect(ROLE_ORDER).toEqual(BUILTIN_ROLE_CATALOG.roles)
  })
  it('keeps reserved permissions ungranted', () => {
    const reserved = new Set(
      BUILTIN_ROLE_CATALOG.permissions
        .filter((permission) => permission.status === 'reserved')
        .map((permission) => permission.key),
    )
    expect(Object.values(BUILTIN_ROLE_CATALOG.grants).flat().some((key) => reserved.has(key))).toBe(false)
  })
})
```

- [ ] **Step 2: Commit/push and verify RED**

```bash
git add crates/server/src/api/openapi.rs crates/server/tests/role_catalog.rs \
  web/src/rbac/builtinRoleCatalog.test.ts
git commit -m "test(rbac): require one catalog across DB OpenAPI and web"
git push -u origin cursor/phase1c-rbac-foundation-6ddb
cargo test -p fileconv-server openapi_role_enums_match_builtin --lib
pnpm --dir web test -- src/rbac/builtinRoleCatalog.test.ts
```

Expected: tests fail because the OpenAPI extension and web consumer do not exist.

- [ ] **Step 3: Implement the three consumers**

Add at OpenAPI top level:

```yaml
x-markhand-builtin-role-catalog: ./builtin-role-catalog.json
```

Add `"resolveJsonModule": true` to `web/tsconfig.json`, then create:

```typescript
import catalog from '../../../crates/server/openapi/builtin-role-catalog.json'
import type { MembershipRole } from '../components/admin/types'

export const BUILTIN_ROLE_CATALOG = catalog
export const ROLE_ORDER = catalog.roles as readonly MembershipRole[]
```

Change `memberPresentation.ts` to import `ROLE_ORDER`; keep only labels/colors in `ROLE_META`. In `role_catalog.rs`, load `fileconv_server::auth::rbac_catalog::load_builtin_role_catalog()` and compare sorted DB rows to the fixture.

- [ ] **Step 4: Commit/push and verify GREEN**

```bash
git add crates/server/openapi/openapi.yaml crates/server/openapi/README.md \
  crates/server/src/api/openapi.rs crates/server/tests/role_catalog.rs \
  web/tsconfig.json web/src/rbac/builtinRoleCatalog.ts \
  web/src/rbac/builtinRoleCatalog.test.ts \
  web/src/components/admin/memberPresentation.ts
git commit -m "feat(rbac): share built-in role catalog across contracts"
git push -u origin cursor/phase1c-rbac-foundation-6ddb
cargo test -p fileconv-server api::openapi --lib
cargo test -p fileconv-server auth::rbac_catalog --lib
pnpm --dir web api:check
pnpm --dir web test -- src/rbac/builtinRoleCatalog.test.ts
```

With PostgreSQL available:

```bash
cargo test -p fileconv-server --test role_catalog -- --include-ignored
```

Expected: fast, web, and DB matrix tests pass.

- [ ] **Step 5: Independent review**

Composer verifies that OpenAPI/web do not contain a copied role-permission matrix and that existing admin owner-tier behavior remains unchanged.

---

### Task 3: Record PR 1 policy dispositions and closure evidence

**Files:**
- Modify: `plans/markhand-web/phase-1c-multi-org-security.md`
- Modify: `plans/markhand-web/backlog/phase-1c/issues/README.md`
- Modify: `docs/markhand-web-risk-register.md`
- Modify: `plans/markhand-web/README.md`
- Modify: `docs/project-roadmap.md`
- Regenerate: `plans/markhand-web/roadmap.html`
- Regenerate: `plans/markhand-web/backlog/github-issues.json`

**Interfaces:**
- Consumes: green `rust`, `web`, and `rust-integration` checks for the exact branch SHA.
- Produces: 1C-01/02/03 `Done`, accepted risk `AR-1C-AUDIT-RETENTION`, and the local/mock embedding condition.

- [ ] **Step 1: Update policy text without claiming unrun evidence**

Make these exact dispositions:

```text
P1C.2: active/reserved matrix follows builtin-role-catalog.json.
P1C.3 remains open for PR 2 ACL semantics.
P1C.5: embedding-token metering is N/A only for local/mock qualifying runtime.
P1C.6: audit retention is deferred to Phase 4 under AR-1C-AUDIT-RETENTION.
AR expiry: before production multi-org or Phase 4 gate, whichever comes first.
```

Keep 1C-01/02/03 `In progress` until CI completes.

- [ ] **Step 2: Verify the branch before requesting CI evidence**

```bash
cargo fmt --all -- --check
cargo metadata --locked --format-version 1 --no-deps
python3 scripts/check-dependency-policy.py
pnpm --dir web format:check
pnpm --dir web lint
pnpm --dir web test
pnpm --dir web api:check
python3 scripts/build-roadmap.py --check
```

- [ ] **Step 3: Push, wait for required checks, and capture evidence**

```bash
git add plans/markhand-web/phase-1c-multi-org-security.md \
  docs/markhand-web-risk-register.md
git commit -m "docs(web): disposition Phase 1C RBAC policy gaps"
git push -u origin cursor/phase1c-rbac-foundation-6ddb
gh run list --branch cursor/phase1c-rbac-foundation-6ddb --workflow ci.yml --limit 10
```

Require `rust`, `web`, and `rust-integration` success. Inspect the integration log to confirm `orgs`, `members`, and `role_catalog` tests executed rather than soft-skipped.

- [ ] **Step 4: Mark 1C-01/02/03 Done and regenerate**

Insert the CI run/job links and tested SHA in the issue catalog, then:

```bash
python3 scripts/build-roadmap.py
python3 scripts/build-roadmap.py --check
python3 scripts/sync-github-issues.py \
  --export-json plans/markhand-web/backlog/github-issues.json
python3 scripts/sync-github-issues.py --dry-run
```

- [ ] **Step 5: Commit/push final PR 1 evidence**

```bash
git add plans/markhand-web/phase-1c-multi-org-security.md \
  plans/markhand-web/backlog/phase-1c/issues/README.md \
  docs/markhand-web-risk-register.md plans/markhand-web/README.md \
  docs/project-roadmap.md plans/markhand-web/roadmap.html \
  plans/markhand-web/backlog/github-issues.json
git commit -m "docs(web): close Phase 1C organization and RBAC foundation"
git push -u origin cursor/phase1c-rbac-foundation-6ddb
```

Composer performs final PR review. Merge PR 1 before creating PR 2.

---

## PR 2 — ACL Resolver, Predicates, and Invalidation

### Task 4: Define canonical ACL semantics and SQL builders

**Files:**
- Create: `crates/server/src/auth/acl.rs`
- Create: `crates/server/src/db/acl_sql.rs`
- Modify: `crates/server/src/auth/mod.rs`
- Modify: `crates/server/src/db/mod.rs`
- Modify: `crates/server/src/db/models.rs`
- Test: unit tests in the two new modules

**Interfaces:**
- Produces: `AccessLevel::satisfies`, `AclPrincipal`, `CollectionAclSnapshot`, `allowed()`, `allowed_collections_sql()`, `acl_predicate_sql()`.
- Consumed by: resolver, upload saga, FTS/hydration, direct operation guards.

- [ ] **Step 1: Write RED semantic matrix tests**

Use the existing `AccessLevel` enum and add tests covering:

```text
inactive/disabled/missing permission always denies
private ignores group/role grants
private accepts owner or sufficient direct-user grant
org accepts active member with base permission
groups accepts sufficient user/group/current-role grant
read < write < admin
read grant never satisfies write/admin
```

The production signature is:

```rust
pub fn allowed(
    principal: &AclPrincipal,
    collection: &CollectionAclSnapshot,
    permission: &str,
    required_access: AccessLevel,
) -> bool;
```

- [ ] **Step 2: Commit/push and verify RED**

```bash
git checkout master
git pull origin master
git checkout -b cursor/phase1c-acl-enforcement-6ddb
git add crates/server/src/auth/acl.rs crates/server/src/db/acl_sql.rs \
  crates/server/src/auth/mod.rs crates/server/src/db/mod.rs \
  crates/server/src/db/models.rs
git commit -m "test(server): define Phase 1C ACL semantics"
git push -u origin cursor/phase1c-acl-enforcement-6ddb
cargo test -p fileconv-server auth::acl --lib
cargo test -p fileconv-server db::acl_sql --lib
```

Expected: tests fail until semantics and SQL builders are implemented.

- [ ] **Step 3: Implement the pure predicate and access ordering**

Add:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AclPrincipal {
    pub org_id: uuid::Uuid,
    pub user_id: uuid::Uuid,
    pub membership_active: bool,
    pub user_disabled: bool,
    pub permissions: std::collections::BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionAclSnapshot {
    pub collection_id: uuid::Uuid,
    pub org_id: uuid::Uuid,
    pub owner_user_id: uuid::Uuid,
    pub visibility: CollectionVisibility,
    pub user_grant: Option<AccessLevel>,
    pub group_grants: Vec<AccessLevel>,
    pub role_grant: Option<AccessLevel>,
}

impl AccessLevel {
    pub const fn rank(self) -> u8 {
        match self {
            Self::Read => 1,
            Self::Write => 2,
            Self::Admin => 3,
        }
    }

    pub const fn satisfies(self, required: Self) -> bool {
        self.rank() >= required.rank()
    }
}

pub fn allowed(
    principal: &AclPrincipal,
    collection: &CollectionAclSnapshot,
    permission: &str,
    required_access: AccessLevel,
) -> bool {
    if !principal.membership_active
        || principal.user_disabled
        || principal.org_id != collection.org_id
        || !principal.permissions.contains(permission)
    {
        return false;
    }
    let grant_allows = |grant: Option<AccessLevel>| {
        grant.is_some_and(|level| level.satisfies(required_access))
    };
    match collection.visibility {
        CollectionVisibility::Private => {
            principal.user_id == collection.owner_user_id
                || grant_allows(collection.user_grant)
        }
        CollectionVisibility::Org => true,
        CollectionVisibility::Groups => {
            principal.user_id == collection.owner_user_id
                || grant_allows(collection.user_grant)
                || collection
                    .group_grants
                    .iter()
                    .copied()
                    .any(|level| level.satisfies(required_access))
                || grant_allows(collection.role_grant)
        }
    }
}
```

`allowed()` must first require active membership, enabled user, org equality, and base permission. Then apply the visibility branches from the approved spec. `acl_sql.rs` emits the same branches and uses a single SQL rank expression:

```sql
CASE access_level WHEN 'read' THEN 1 WHEN 'write' THEN 2 WHEN 'admin' THEN 3 ELSE 0 END
```

Expose these builders:

```rust
pub fn allowed_collections_sql(
    org_id_param: &str,
    user_id_param: &str,
    permission_param: &str,
    required_access_param: &str,
) -> String;

pub fn acl_predicate_sql(
    org_id_expr: &str,
    collection_id_expr: &str,
    user_id_param: &str,
    permission_param: &str,
    required_access_param: &str,
) -> String;
```

- [ ] **Step 4: Commit/push and verify GREEN**

```bash
git add crates/server/src/auth/acl.rs crates/server/src/db/acl_sql.rs \
  crates/server/src/db/models.rs
git commit -m "feat(server): implement canonical collection ACL semantics"
git push -u origin cursor/phase1c-acl-enforcement-6ddb
cargo test -p fileconv-server auth::acl --lib
cargo test -p fileconv-server db::acl_sql --lib
```

Grok reviews semantic and SQL equivalence before Task 5.

---

### Task 5: Add concurrency-safe no-dormant-grant migration

**Files:**
- Create: `crates/server/migrations/0036_expand_acl_groups_invariants.sql`
- Modify: `crates/server/migrations/manifest.json`
- Modify: `crates/server/src/database.rs`
- Modify: `crates/server/tests/schema_migrations.rs`
- Create: `crates/server/tests/acl_concurrency.rs`

**Interfaces:**
- Produces: no-dormant DB invariant, parent-row lock protocol, ACL-version triggers for group/role grants and group membership.

- [ ] **Step 1: Write RED migration-shape and concurrency tests**

Tests must prove:

```text
group/role grant on private or org visibility is rejected
groups visibility accepts grant
concurrent grant-vs-private flip cannot leave dormant rows
groups-to-private fails while grants remain
acl_version bumps on group grant, role grant, and group membership changes
```

- [ ] **Step 2: Commit/push and verify RED**

```bash
git add crates/server/tests/acl_concurrency.rs \
  crates/server/tests/schema_migrations.rs crates/server/src/database.rs
git commit -m "test(server): expose dormant ACL grant races"
git push -u origin cursor/phase1c-acl-enforcement-6ddb
cargo test -p fileconv-server database::tests::acl_groups_invariants --lib
```

Expected: migration shape test fails because `0036` does not exist.

- [ ] **Step 3: Implement migration `0036`**

The migration must:

1. Fail preflight with sorted collection IDs if group/role grants exist on non-`groups` collections.
2. Add `BEFORE INSERT OR UPDATE` triggers on both grant tables. The trigger locks the parent with `SELECT visibility FROM collections WHERE org_id = NEW.org_id AND id = NEW.collection_id FOR NO KEY UPDATE` and then requires `visibility = 'groups'`.
3. Add `BEFORE UPDATE OF visibility` on `collections`; when `NEW.visibility <> 'groups'`, raise if group/role grants remain.
4. Add ACL-version bump triggers on `collection_group_access`, `collection_role_access`, and `group_memberships`.

Use this lock order in every visibility writer:

```text
lock parent collection FOR NO KEY UPDATE
delete group grants
delete role grants
update visibility
commit
```

Update `schema_migrations.rs` to set the POC fixture collection to `groups` before inserting its group grant. Do not silently delete pre-existing dormant rows in the migration.

- [ ] **Step 4: Update checksum, commit/push, and verify GREEN**

```bash
python3 scripts/check-migration-manifest.py --write-manifest
python3 scripts/check-migration-manifest.py --check
git add crates/server/migrations/0036_expand_acl_groups_invariants.sql \
  crates/server/migrations/manifest.json crates/server/src/database.rs \
  crates/server/tests/schema_migrations.rs crates/server/tests/acl_concurrency.rs
git commit -m "feat(server): enforce concurrency-safe ACL grant invariants"
git push -u origin cursor/phase1c-acl-enforcement-6ddb
cargo test -p fileconv-server database::tests::acl_groups_invariants --lib
```

With PostgreSQL:

```bash
cargo test -p fileconv-server --test acl_concurrency -- --include-ignored --test-threads=1
cargo test -p fileconv-server --test schema_migrations -- --include-ignored --test-threads=1
```

Expected: no interleaving leaves group/role grants on non-`groups` collections.

---

### Task 6: Wire resolver, search, cache, and containment

**Files:**
- Modify: `crates/server/src/auth/permissions.rs`
- Modify: `crates/server/src/db/search.rs`
- Modify: `crates/server/src/services/upload/saga.rs`
- Modify: `crates/server/src/services/acl_mutate.rs`
- Create: `crates/server/tests/common/acl_fixture.rs`
- Create: `crates/server/tests/acl_resolver.rs`
- Create: `crates/server/tests/acl_equivalence.rs`
- Modify: `crates/server/tests/common/mod.rs`
- Modify: `crates/server/tests/acl_cache.rs`
- Modify: `crates/server/tests/retrieval.rs`
- Audit and modify fixture call sites returned by: `rg -l 'seed_user_with_permissions' crates/server/tests`

**Interfaces:**
- `OrgContext.allowed_collection_ids` becomes exactly the `(qa.query, read)` projection.
- Write/admin services use `acl_predicate_sql(org_expr, collection_expr, user_param, permission_param, required_access_param)` rather than inferring from that read projection.
- Produces `require_operation_collection_access_on_txn(txn, ctx, collection_id, permission, required_access) -> Result<(), ResolveError>` for PR 3 services.

- [ ] **Step 1: Write RED DB tests**

Add named tests:

```text
groups_visibility_group_grant_allows_member_without_user_grant
private_visibility_ignores_group_and_role_grants
resolver_matches_sql_predicate_for_acl_fixture_matrix
read_grant_does_not_satisfy_write_or_admin
group_membership_revoke_invalidates_cached_context
containment_removes_group_role_grants_but_preserves_other_user_grants
```

Before changing the resolver, audit every `seed_user_with_permissions` call. Add
`qa.query` only to fixtures whose actor is expected to obtain non-empty collection
scope; preserve and annotate fixtures intentionally proving missing-query denial. This
prevents the new `(qa.query, read)` projection from turning unrelated integration
tests red for the wrong reason.

- [ ] **Step 2: Commit/push and verify RED**

```bash
git add crates/server/tests/common/acl_fixture.rs \
  crates/server/tests/common/mod.rs crates/server/tests/acl_resolver.rs \
  crates/server/tests/acl_equivalence.rs crates/server/tests/acl_cache.rs \
  crates/server/tests/retrieval.rs
git commit -m "test(server): require equivalent ACL resolution and SQL enforcement"
git push -u origin cursor/phase1c-acl-enforcement-6ddb
cargo test -p fileconv-server --test acl_resolver -- --include-ignored
```

Expected: groups collection is absent from the current resolver.

- [ ] **Step 3: Implement shared resolution and predicate use**

Refactor both `resolve_org_context` variants and `upload::saga::reload_principal_locked` to use the same `allowed_collections_sql()` with:

```text
permission = qa.query
required_access = read
```

Move the current `db/search.rs::acl_predicate_sql` implementation to `db/acl_sql.rs`; preserve the source-shape test and pass `read` for FTS/hydration/conflict queries. Add `write/admin` parameters for operation guards used by PR 3.

Add:

```rust
pub async fn require_operation_collection_access_on_txn(
    txn: &tokio_postgres::Transaction<'_>,
    ctx: &OrgContext,
    collection_id: uuid::Uuid,
    permission: &str,
    required_access: AccessLevel,
) -> Result<(), ResolveError> {
    require_permission(ctx, permission)?;
    let sql = format!(
        "SELECT EXISTS (
           SELECT 1 FROM collections c
           WHERE c.org_id = $1 AND c.id = $2 AND c.deleted_at IS NULL AND ({})
         )",
        crate::db::acl_sql::acl_predicate_sql("c.org_id", "c.id", "$3", "$4", "$5")
    );
    let allowed: bool = txn
        .query_one(
            &sql,
            &[&ctx.org_id(), &collection_id, &ctx.user_id(), &permission, &required_access.as_str()],
        )
        .await
        .map_err(|_| ResolveError::Database)?
        .get(0);
    if allowed {
        Ok(())
    } else {
        Err(ResolveError::CollectionDenied)
    }
}
```

Add `AccessLevel::as_str()` beside `rank()` and `satisfies()`.

In `revoke_collection_access_for_principal`, first lock the collection row `FOR NO KEY UPDATE`, delete all group/role grants, update to private/new owner, then delete only the target user's direct grant. Preserve other users' direct grants.

- [ ] **Step 4: Commit/push and verify GREEN**

```bash
git add crates/server/src/auth/permissions.rs crates/server/src/db/search.rs \
  crates/server/src/services/upload/saga.rs \
  crates/server/src/services/acl_mutate.rs \
  crates/server/tests/acl_resolver.rs crates/server/tests/acl_equivalence.rs \
  crates/server/tests/acl_cache.rs crates/server/tests/retrieval.rs
git commit -m "feat(server): enforce group role and access-level ACLs"
git push -u origin cursor/phase1c-acl-enforcement-6ddb
cargo test -p fileconv-server auth::acl --lib
cargo test -p fileconv-server db::acl_sql --lib
cargo test -p fileconv-server db::search --lib
```

With PostgreSQL:

```bash
cargo test -p fileconv-server --test acl_resolver -- --include-ignored --test-threads=1
cargo test -p fileconv-server --test acl_equivalence -- --include-ignored --test-threads=1
cargo test -p fileconv-server --test acl_cache -- --include-ignored --test-threads=1
cargo test -p fileconv-server --test retrieval \
  fts_candidate_leg_and_hydration_deny_acl_and_suspended_membership \
  -- --include-ignored
```

Expected: resolver and SQL sets are equal for every fixture state.

- [ ] **Step 5: Review and close PR 2 evidence**

Grok reviews migration concurrency, Rust/SQL equivalence, and containment. After `rust` and `rust-integration` succeed, update P1C.3 wording, mark 1C-05/06 Done with CI links, regenerate roadmap/issue JSON, commit, push, and merge before PR 3.

---

## PR 3 — Guard Inventory and Operational Identities

### Task 7: Add a machine-checked guard and audit inventory

**Files:**
- Create: `crates/server/openapi/guard-inventory.json`
- Create: `crates/server/src/auth/guard_inventory.rs`
- Modify: `crates/server/src/auth/mod.rs`
- Modify: `crates/server/src/api/openapi.rs`
- Test: unit tests in `guard_inventory.rs`

**Interfaces:**
- Each OpenAPI operation is classified as `public`, `authenticated`, `permission`, `capability`, or `system`.
- Permission rows contain `operationId`, `permission`, route method/path, `serviceEntry`, `identityKind`, and audit disposition.
- `requiredCollectionAccess` is derived from `builtin-role-catalog.json`, never stored in the guard JSON.

- [ ] **Step 1: Write RED completeness tests**

Tests extract every OpenAPI `operationId` and every `ROUTE_INVENTORY` method/path, then assert exactly one guard row. Permission rows must reference active catalog keys and a route/service pair; reserved keys are rejected. Mutation rows must contain either an audit action or a documented non-sensitive `na` reason.

- [ ] **Step 2: Commit/push and verify RED**

```bash
git checkout master
git pull origin master
git checkout -b cursor/phase1c-guard-identities-6ddb
git add crates/server/src/auth/guard_inventory.rs \
  crates/server/src/auth/mod.rs crates/server/src/api/openapi.rs
git commit -m "test(server): require complete route and service guard inventory"
git push -u origin cursor/phase1c-guard-identities-6ddb
cargo test -p fileconv-server auth::guard_inventory --lib
```

Expected: missing fixture/completeness failures enumerate current operations.

- [ ] **Step 3: Fill the canonical inventory**

Use current `operationId` values from `openapi.yaml`. At minimum, map:

```text
doc.upload -> create upload/collection, update/assign/reindex write services
doc.delete -> document and collection delete admin services
doc.publish -> publish version write service
qa.query -> list/search/ask/citation/preview/download/status/SSE read services
qa.history -> historical version/diff read services
member.manage -> member/invite/usage admin services
audit.view -> audit query service
jobs.system -> worker/job system services
doc.quarantine.review -> approve-intake write service
```

Classify auth/login/refresh/health as public/authenticated rather than inventing permissions. Classify capability redemption as capability-authenticated. Derive collection access from the RBAC fixture during validation.

Each permission row follows this shape; `requiredCollectionAccess` is intentionally absent:

```json
{
  "operationId": "publishDocumentVersion",
  "authzKind": "permission",
  "permission": "doc.publish",
  "route": {"method": "post", "path": "/documents/{documentId}/versions/{versionId}/publish"},
  "serviceEntry": "services::publish::publish_version",
  "identityKind": "user",
  "mutation": true,
  "audit": {"status": "required", "action": "document.publish"}
}
```

- [ ] **Step 4: Commit/push and verify GREEN**

```bash
git add crates/server/openapi/guard-inventory.json \
  crates/server/src/auth/guard_inventory.rs \
  crates/server/src/auth/mod.rs crates/server/src/api/openapi.rs
git commit -m "feat(server): register route service and audit guards"
git push -u origin cursor/phase1c-guard-identities-6ddb
cargo test -p fileconv-server auth::guard_inventory --lib
cargo test -p fileconv-server api::openapi --lib
```

Composer reviews inventory completeness and absence of reserved operations.

---

### Task 8: Enforce route and direct-service authorization

**Files:**
- Modify: `crates/server/src/services/deletion.rs`
- Create or modify: `crates/server/src/services/publish.rs`
- Modify: `crates/server/src/services/members.rs`
- Create or modify: `crates/server/src/services/audit_query.rs`
- Modify: `crates/server/src/services/upload/saga.rs`
- Modify: matching route modules under `crates/server/src/routes/`
- Modify: `crates/server/src/services/mod.rs`
- Test: `crates/server/tests/api_http_contracts.rs`
- Test: `crates/server/tests/members.rs`
- Test: `crates/server/tests/audit_read.rs`
- Test: focused direct-service integration tests

**Interfaces:**
- Route guards preserve HTTP 403/404 contracts.
- Direct services re-check permission and required collection access before side effects.
- Publish moves from direct DB route call into a transaction-owning service that records audit.

- [ ] **Step 1: Add RED direct-service misuse tests**

For each active permission, call the service directly with an otherwise valid `OrgContext` missing that permission. Assert the specific permission-denied error and zero DB/object/job/audit side effects. Include:

```text
doc.delete, doc.publish, member.manage, audit.view, jobs.system
```

- [ ] **Step 2: Commit/push and verify RED**

```bash
git add crates/server/tests/api_http_contracts.rs crates/server/tests/members.rs \
  crates/server/tests/audit_read.rs
git commit -m "test(server): expose direct service authorization bypasses"
git push -u origin cursor/phase1c-guard-identities-6ddb
cargo test -p fileconv-server --test members \
  member_manage_permission_required_for_patch_and_delete -- --include-ignored
```

Expected: at least deletion/publish/member/audit direct calls bypass the intended service guard.

- [ ] **Step 3: Add minimal guards and publish service**

At every service entry, call:

```rust
require_permission(ctx, permission_code)?;
require_operation_collection_access_on_txn(
    txn,
    ctx,
    collection_id,
    permission_code,
    required_access,
).await?;
```

Org-level operations omit the collection check. `publish_version` begins one transaction, checks `doc.publish/write`, publishes, records `document.publish` through the audit allowlist, and commits. Routes retain their guards and map permission failure to 403; path IDOR remains 404 after authorized lookup.

- [ ] **Step 4: Commit/push and verify GREEN**

```bash
git add crates/server/src/services crates/server/src/routes \
  crates/server/tests/api_http_contracts.rs crates/server/tests/members.rs \
  crates/server/tests/audit_read.rs
git commit -m "feat(server): enforce authorization at route and service layers"
git push -u origin cursor/phase1c-guard-identities-6ddb
cargo test -p fileconv-server auth::guard_inventory --lib
```

Run relevant integration binaries with `--include-ignored`; expected: all allow/deny pairs pass and no side-effect occurs on direct denial.

- [ ] **Step 5: Independent review**

Composer checks no existence oracle was introduced, transaction/audit ordering is atomic, and every guard-inventory service entry is enforced.

---

### Task 9: Add least-privilege worker identities and POC worker DB role

**Files:**
- Modify: `crates/server/src/bin/worker.rs`
- Modify: `crates/server/src/config.rs`
- Modify: `deploy/poc/postgres-init.sh`
- Modify: `deploy/scripts/bootstrap-server-role.sh`
- Modify: `deploy/compose.poc.yml`
- Modify: `deploy/.env.example`
- Modify: `deploy/scripts/poc-isolation-smoke.sh`
- Test: `crates/server/tests/pool_worker_defense.rs`
- Test: unit tests in `worker.rs`/`config.rs`

**Interfaces:**
- `worker_permissions(kind: &str) -> Result<&'static [&'static str], String>`.
- Worker kind is parsed before `OrgContext` construction.
- Qualifying POC workers must use `MARKHAND_WORKER_DATABASE_URL` as `markhand_worker`; app-role fallback remains dev compatibility only.

- [ ] **Step 1: Add RED identity/config tests**

Pin:

```text
convert/index/embedding -> jobs.system + doc.upload
delete/reconcile -> jobs.system + doc.delete
unknown kind -> configuration error
worker contexts contain exact permissions, never member.manage/audit.view
phase1c qualifying compose contains MARKHAND_WORKER_DATABASE_URL for every worker
runtime role is non-superuser, non-BYPASSRLS, and not table owner
```

- [ ] **Step 2: Commit/push and verify RED**

```bash
git add crates/server/src/bin/worker.rs crates/server/src/config.rs \
  crates/server/tests/pool_worker_defense.rs \
  deploy/scripts/poc-isolation-smoke.sh
git commit -m "test(server): require least-privilege worker identities"
git push -u origin cursor/phase1c-guard-identities-6ddb
cargo test -p fileconv-server --bin fileconv-worker worker_permissions
bash deploy/scripts/poc-isolation-smoke.sh
```

Expected: empty worker permissions and missing dedicated POC URL fail.

- [ ] **Step 3: Implement identity and provisioning**

Move kind parsing before context construction and call `worker_permissions`. Provision `markhand_worker` idempotently from secret env input; never put its password in SQL/migrations/logs. Keep migration `0035` grant behavior. Set worker services to the dedicated URL in `compose.poc.yml`.

- [ ] **Step 4: Commit/push and verify GREEN**

```bash
git add crates/server/src/bin/worker.rs crates/server/src/config.rs \
  deploy/poc/postgres-init.sh deploy/scripts/bootstrap-server-role.sh \
  deploy/compose.poc.yml deploy/.env.example \
  deploy/scripts/poc-isolation-smoke.sh \
  crates/server/tests/pool_worker_defense.rs
git commit -m "feat(server): run POC workers with least privilege"
git push -u origin cursor/phase1c-guard-identities-6ddb
cargo test -p fileconv-server --bin fileconv-worker worker_permissions
bash deploy/scripts/poc-isolation-smoke.sh
```

With PostgreSQL:

```bash
cargo test -p fileconv-server --test pool_worker_defense -- --include-ignored
```

Expected: CI half of 1C-08 passes; deployed proof remains open for PR 5.

---

### Task 10: Close CI-verifiable enforcement issues

**Files:**
- Modify: `plans/markhand-web/backlog/phase-1c/issues/README.md`
- Modify: `plans/markhand-web/README.md`
- Modify: `docs/project-roadmap.md`
- Regenerate: roadmap and GitHub issue JSON

**Interfaces:**
- Closes: 1C-04, 1C-07, 1C-09, 1C-10.
- Records: 1C-08 CI half; 1C-11 closes only because owner approved `AR-1C-AUDIT-RETENTION` and audit coverage is green.

- [ ] **Step 1: Run full preflight and targeted evidence**

```bash
cargo fmt --all -- --check
cargo metadata --locked --format-version 1 --no-deps
python3 scripts/check-dependency-policy.py
bash scripts/run-rust-ci-fast.sh
bash deploy/scripts/poc-isolation-smoke.sh
```

Require GitHub `rust-integration` success for `pool_worker_defense`, `quota`, `noisy_neighbor`, `storage`, `audit_read`, `members`, and direct-service tests.

- [ ] **Step 2: Update only evidence-backed statuses**

Add exact run/job links and state:

```text
1C-04 Done
1C-07 Done
1C-08 In progress — CI half complete, deployed half PR 5
1C-09 Done
1C-10 Done
1C-11 Done — audit coverage + approved AR-1C-AUDIT-RETENTION
```

- [ ] **Step 3: Regenerate, commit, push, and review**

```bash
python3 scripts/build-roadmap.py
python3 scripts/build-roadmap.py --check
python3 scripts/sync-github-issues.py \
  --export-json plans/markhand-web/backlog/github-issues.json
git add plans/markhand-web/backlog/phase-1c/issues/README.md \
  plans/markhand-web/README.md docs/project-roadmap.md \
  plans/markhand-web/roadmap.html \
  plans/markhand-web/backlog/github-issues.json
git commit -m "docs(web): record Phase 1C enforcement evidence"
git push -u origin cursor/phase1c-guard-identities-6ddb
```

Composer performs whole-PR review. Merge PR 3 before PR 4.

---

## PR 4 — Unified Multi-Org Denial Suite

### Task 11: Eliminate integration soft-green prerequisites

**Files:**
- Modify: `crates/server/tests/common/mod.rs`
- Modify: `.github/workflows/ci.yml`
- Test: `crates/server/tests/api_http_contracts.rs`
- Test: unit/source tests in `tests/common/mod.rs` consumers

**Interfaces:**
- Produces: `markhand_test_required()` and strict `take_live()`.

- [ ] **Step 1: Add RED required-mode tests**

Require:

```rust
pub fn markhand_test_required() -> bool {
    std::env::var("MARKHAND_TEST_REQUIRED").ok().as_deref() == Some("1")
        || markhand_e2e_required()
}
```

Tests use an environment lock and assert missing prerequisites panic only in required mode.

- [ ] **Step 2: Commit/push and verify RED**

```bash
git checkout master
git pull origin master
git checkout -b cursor/phase1c-denial-suite-6ddb
git add crates/server/tests/common/mod.rs .github/workflows/ci.yml
git commit -m "test(server): forbid integration prerequisite soft passes"
git push -u origin cursor/phase1c-denial-suite-6ddb
cargo test -p fileconv-server --test api_http_contracts required_mode
```

Expected: current helper only honors `MARKHAND_E2E`.

- [ ] **Step 3: Implement strict mode and CI environment**

Update `take_live()` to panic with `MARKHAND_TEST_REQUIRED=1 requires {name}`. Set `MARKHAND_TEST_REQUIRED: "1"` in `rust-integration`. Keep unset local ignored tests skippable with explicit stderr.

- [ ] **Step 4: Commit/push and verify GREEN**

```bash
git add crates/server/tests/common/mod.rs .github/workflows/ci.yml
git commit -m "test(server): require live dependencies in integration CI"
git push -u origin cursor/phase1c-denial-suite-6ddb
cargo test -p fileconv-server --test api_http_contracts required_mode
```

Grok reviews helper coverage and workflow variables.

---

### Task 12: Build the shared multi-org world and denial manifest

**Files:**
- Create: `crates/server/tests/fixtures/multi-org-denial.fixture.json`
- Create: `crates/server/tests/fixtures/multi-org-denial.manifest.json`
- Create: `crates/server/tests/fixtures/multi-org-denial.na-evidence.json`
- Create: `crates/server/tests/common/multi_org_denial.rs`
- Create: `crates/server/tests/multi_org_denial.rs`
- Create: `crates/server/tests/multi_org_denial_manifest.rs`
- Modify: `crates/server/tests/common/mod.rs`

**Interfaces:**
- `MultiOrgDenialWorld::boot()` creates two orgs, three users per org, private/org/groups collections, duplicate names, indexed foreign markers, and pre-revoke tokens.
- `assert_denial_no_leak(response, foreign_markers)` checks status/error and scans body/headers for foreign IDs, names, keys, and marker strings.
- Manifest rows contain `id`, `binary`, `testName`, `operationId`, `guardInventoryRef`, `layer`, and `status`.

- [ ] **Step 1: Add RED fixture and manifest validators**

Validator rules:

```text
every guard-inventory business operation has a denial row
every ROUTE_INVENTORY business route joins through guard inventory
every executable binary/testName exists in test source
every N/A row has source-scan or capability-substitution evidence
N/A becomes invalid when its operation appears
```

- [ ] **Step 2: Commit/push and verify RED**

```bash
git add crates/server/tests/fixtures crates/server/tests/common/multi_org_denial.rs \
  crates/server/tests/common/mod.rs crates/server/tests/multi_org_denial.rs \
  crates/server/tests/multi_org_denial_manifest.rs
git commit -m "test(server): define connected multi-org denial manifest"
git push -u origin cursor/phase1c-denial-suite-6ddb
cargo test -p fileconv-server --test multi_org_denial_manifest
```

Expected: missing rows/test references are listed deterministically.

- [ ] **Step 3: Implement fixture, manifest, and source join**

N/A rows are exactly:

```text
export route absent
autocomplete route absent
signed URL replaced by capability-token tests
settings/intel/PII permissions reserved with no runtime operation
embedding token metering disallowed by qualifying local/mock profile
```

All existing cross-org tests are referenced by binary/test name; do not copy their logic into the new test. Add new shared-world tests for gaps.

- [ ] **Step 4: Commit/push and verify GREEN**

```bash
git add crates/server/tests/fixtures crates/server/tests/common \
  crates/server/tests/multi_org_denial.rs \
  crates/server/tests/multi_org_denial_manifest.rs
git commit -m "test(server): connect Phase 1C cross-org denial evidence"
git push -u origin cursor/phase1c-denial-suite-6ddb
cargo test -p fileconv-server --test multi_org_denial_manifest
```

Expected: manifest joins guard/route/test sources with zero validation errors.

---

### Task 13: Add missing indexed and cache denial cases

**Files:**
- Modify: `crates/server/tests/multi_org_denial.rs`
- Reuse: `crates/server/tests/common/worker_pipeline.rs`
- Reuse/modify: `crates/server/tests/api_http_contracts.rs`
- Reuse/modify: `crates/server/tests/acl_cache.rs`
- Reuse/modify: `crates/server/tests/sse_stream_readiness.rs`

**Interfaces:**
- Produces connected evidence for indexed FTS/Q&A, duplicate names, org-switch cache, stale token, preview/download, SSE, and in-flight revoke.

- [ ] **Step 1: Add RED exploit-first cases**

Add exact tests:

```text
indexed_fts_and_ask_never_return_foreign_marker
duplicate_names_across_orgs_do_not_create_an_oracle
org_switch_never_reuses_previous_org_cache_scope
pre_revoke_tokens_fail_after_downgrade_suspend_and_remove
preview_download_job_and_sse_hide_foreign_ids
in_flight_ask_emits_no_content_after_acl_revoke
```

- [ ] **Step 2: Commit/push and verify RED**

```bash
git add crates/server/tests/multi_org_denial.rs
git commit -m "test(server): expose remaining multi-org denial gaps"
git push -u origin cursor/phase1c-denial-suite-6ddb
cargo build -p fileconv-cli --no-default-features
cargo test -p fileconv-server --test multi_org_denial -- --include-ignored --nocapture
```

Expected: tests fail before the world builder produces indexed artifacts and stale-token transitions.

- [ ] **Step 3: Implement only fixture/helper support**

Use production HTTP routes and `WorkerPipeline`; do not add test-only production bypasses. Assert zero foreign markers in every successful or denied response. Preserve body-scope 403 and path-IDOR 404 semantics.

- [ ] **Step 4: Commit/push and verify GREEN**

```bash
git add crates/server/tests/multi_org_denial.rs \
  crates/server/tests/common/multi_org_denial.rs \
  crates/server/tests/common/worker_pipeline.rs \
  crates/server/tests/api_http_contracts.rs \
  crates/server/tests/acl_cache.rs \
  crates/server/tests/sse_stream_readiness.rs
git commit -m "test(server): complete unified multi-org denial coverage"
git push -u origin cursor/phase1c-denial-suite-6ddb
cargo build -p fileconv-cli --no-default-features
cargo test -p fileconv-server --test multi_org_denial -- --include-ignored --nocapture
```

Expected: zero leakage assertions pass.

---

### Task 14: Run the connected suite in CI and record the 1C-12 half-gate

**Files:**
- Create: `scripts/run-phase1c-denial-suite.py`
- Modify: `.github/workflows/ci.yml`
- Create/generated: `bench/markhand_web/reports/phase-1c-denial/manifest-run.json`
- Modify: Phase 1C catalog/status artifacts

**Interfaces:**
- Runner reads the manifest, groups executable rows by integration binary, runs each unique binary with `--include-ignored`, and writes a sanitized manifest/hash/result artifact.

- [ ] **Step 1: Write RED runner self-tests**

Self-tests cover unknown binary, missing test source, nonzero child exit, foreign marker finding, missing required env, and deterministic JSON output.

Commit/push the runner skeleton before executing RED:

```bash
git add scripts/run-phase1c-denial-suite.py
git commit -m "test(ci): define Phase 1C denial runner contract"
git push -u origin cursor/phase1c-denial-suite-6ddb
python3 scripts/run-phase1c-denial-suite.py --self-test
```

Expected: self-tests fail because subprocess execution, redaction, and deterministic
report assembly are not implemented.

- [ ] **Step 2: Implement runner and CI artifact upload**

The `rust-integration` job runs:

```bash
python3 scripts/run-phase1c-denial-suite.py \
  --manifest crates/server/tests/fixtures/multi-org-denial.manifest.json \
  --output "$RUNNER_TEMP/phase1c-denial/manifest-run.json"
```

Upload the output even on failure. The artifact records full git SHA, manifest SHA-256, executable/N/A counts, binaries run, failures, and `leakageCount`.
Name the workflow artifact `phase1c-denial-${{ github.sha }}` so Step 4 can fetch the
evidence for the exact commit.

Run:

```bash
python3 scripts/run-phase1c-denial-suite.py --self-test
```

Expected: all runner self-tests pass.

- [ ] **Step 3: Commit/push and require CI success**

```bash
git add scripts/run-phase1c-denial-suite.py .github/workflows/ci.yml
git commit -m "ci(server): run connected Phase 1C denial suite"
git push -u origin cursor/phase1c-denial-suite-6ddb
```

The manifest runner intentionally re-runs the referenced integration binaries after
the broad `rust-integration` command. The duplicate execution is accepted because it
produces a manifest-scoped result and detects test-selection drift. Require
`rust-integration` and artifact generation with `leakageCount = 0`.

- [ ] **Step 4: Download evidence and record the half-gate**

Download the successful artifact:

```bash
RUN_ID="$(gh run list --branch cursor/phase1c-denial-suite-6ddb \
  --workflow ci.yml --status success --limit 1 \
  --json databaseId --jq '.[0].databaseId')"
rm -rf /tmp/phase1c-denial-artifact
gh run download "$RUN_ID" \
  -n "phase1c-denial-$(git rev-parse HEAD)" \
  -D /tmp/phase1c-denial-artifact
mkdir -p bench/markhand_web/reports/phase-1c-denial
cp /tmp/phase1c-denial-artifact/manifest-run.json \
  bench/markhand_web/reports/phase-1c-denial/manifest-run.json
python3 - <<'PY'
import json
from pathlib import Path
p = Path("bench/markhand_web/reports/phase-1c-denial/manifest-run.json")
report = json.loads(p.read_text())
assert report["leakageCount"] == 0, report["leakageCount"]
assert not report["failures"], report["failures"]
PY
```

Keep 1C-12 `In progress`; state “CI half complete, deployed half pending PR 5” with
run/job links. Regenerate roadmap and issue JSON, then commit/push the downloaded
sanitized report and status artifacts.

- [ ] **Step 5: Whole-PR review**

Grok verifies the runner artifact against the exact branch SHA, performs whole-PR
review, and requires corrections before merge. Merge PR 4 before PR 5.

---

## PR 5 — Phase 1C Security/Load Gate

### Task 15: Define the G1C environment, thresholds, registry, and report schema

**Files:**
- Create: `bench/markhand_web/environments/phase1c-multi-org-poc.yaml`
- Create: `bench/markhand_web/workloads/phase1c-multi-org.yaml`
- Create: `bench/markhand_web/schema/phase1c-gate-report.schema.json`
- Modify: `bench/markhand_web/gates.yaml`
- Modify: `bench/markhand_web/schema/gates.schema.json`
- Modify: `bench/markhand_web/schema/environment.schema.json`
- Modify: `scripts/check-markhand-gates.py`
- Modify: `docs/markhand-web-sla-targets.md`
- Test: validator unit tests in `scripts/check-markhand-gates.py`

**Interfaces:**
- Adds `G1C-SEC` and `block-phase-1c`.
- Defines exact POC qualification thresholds:

```text
cross_tenant_leakage_count == 0
post_commit_stale_authorizations == 0
membership_acl_revoke_max_ms <= 3000
quota_drift_after_recovery == 0
quiet_org_query_p95_ms <= 500
starvation_events == 0
admin_mutation_audit_coverage_ratio == 1.0
worker_dedicated_role_verified == 1
undispositioned_high_critical_count == 0
```

- [ ] **Step 1: Add RED registry/schema tests**

Require the new environment ID, all metrics, approved owner/approver fields, evidence paths, and `failureDisposition = block-phase-1c`. A report with `targetMatch=false`, missing worker proof, or missing threshold decision must fail.

- [ ] **Step 2: Commit/push and verify RED**

```bash
git checkout master
git pull origin master
git checkout -b cursor/phase1c-security-gate-6ddb
git add bench/markhand_web/schema/phase1c-gate-report.schema.json \
  scripts/check-markhand-gates.py
git commit -m "test(gate): define Phase 1C security report contract"
git push -u origin cursor/phase1c-security-gate-6ddb
python3 scripts/check-markhand-gates.py --self-test
```

Expected: the new G1C validator self-test cases fail because the validator does not
yet reject the new family, disposition, environment, and report invariants.

- [ ] **Step 3: Add profile, workload, thresholds, and gate rows**

Copy all required hardware/fingerprint fields from `poc-compose.yaml`, then set:

```text
environmentId = phase1c-multi-org-poc
orgCount = 2
embeddingProfile = mock
requiresDedicatedWorkerRole = true
requiresWorkerDatabaseUrl = true
```

Keep the `.yaml` files JSON-compatible because
`scripts/check-markhand-gates.py::load_json_yaml` parses JSON. Add these four Phase 1C
profile fields to `environment.schema.json` so the schema validates the qualifying
environment rather than silently accepting undocumented properties.

Add separate `G1C-SEC-*` rows for leakage, revoke, ACL cache, quota recovery, noisy neighbor, audit coverage, worker role, container vulnerabilities, stale tokens, and Qdrant fail-closed. Record security-owner and operations-owner approval in the SLA/registry before the qualifying run.

- [ ] **Step 4: Commit/push and verify GREEN**

```bash
git add bench/markhand_web/environments/phase1c-multi-org-poc.yaml \
  bench/markhand_web/workloads/phase1c-multi-org.yaml \
  bench/markhand_web/schema/phase1c-gate-report.schema.json \
  bench/markhand_web/gates.yaml bench/markhand_web/schema/gates.schema.json \
  bench/markhand_web/schema/environment.schema.json \
  scripts/check-markhand-gates.py docs/markhand-web-sla-targets.md
git commit -m "feat(gate): register Phase 1C security qualification"
git push -u origin cursor/phase1c-security-gate-6ddb
python3 scripts/check-markhand-gates.py --self-test
python3 scripts/check-markhand-gates.py
```

Composer reviews threshold provenance and confirms this gate makes no production-scale claim.

---

### Task 16: Build the deployed harness and worker-role proof

**Files:**
- Create: `deploy/scripts/phase1c-multi-org-seed.sh`
- Create: `deploy/scripts/g1c-security-gate.sh`
- Create: `bench/markhand_web/scripts/run_phase1c_gate.py`
- Create: `bench/markhand_web/scripts/test_run_phase1c_gate.py`
- Create: `crates/server/tests/e2e_phase1c_gate.rs`
- Modify: `.github/workflows/ci.yml`
- Modify: `deploy/compose.poc.yml`
- Modify: `deploy/.env.example`
- Modify: `deploy/poc/images.lock.json`
- Create: `docs/runbooks/phase-1c/g1c-security-gate.md`

**Interfaces:**
- Produces sanitized `phase-1c-gate.json` and per-gate evidence.
- Proves worker runtime role, not merely compose text.
- Uses a digest-pinned latest stable Trivy image recorded in `images.lock.json`.

- [ ] **Step 1: Add RED harness validator tests**

Reject:

```text
status pass with targetMatch false
worker runtime role markhand_app
superuser or BYPASSRLS worker
cloud/shared embedding profile
leakage > 0
undispositioned high/critical finding
secret or absolute path in report
missing P1C.8 evidence mapping
```

Declare one ignored test function named exactly `e2e_phase1c_gate` and opt it in through
`MARKHAND_PHASE1C_GATE=1`. In the normal `rust-integration` command add
`--skip e2e_phase1c_gate`; the dedicated G1C job runs this binary explicitly with the
flag, report path, and `--ignored`. This explicit skip is not a prerequisite soft-pass.

- [ ] **Step 2: Commit/push and verify RED**

```bash
git add bench/markhand_web/scripts/run_phase1c_gate.py \
  bench/markhand_web/scripts/test_run_phase1c_gate.py \
  crates/server/tests/e2e_phase1c_gate.rs .github/workflows/ci.yml
git commit -m "test(gate): require complete Phase 1C deployed evidence"
git push -u origin cursor/phase1c-security-gate-6ddb
python3 bench/markhand_web/scripts/test_run_phase1c_gate.py
```

Expected: missing report/harness behavior fails.

- [ ] **Step 3: Implement seed, probes, and report assembly**

The harness:

1. Verifies `MARKHAND_TEST_REQUIRED=1` and mock/local embedding.
2. Seeds two orgs and users through production APIs.
3. Runs the PR 4 denial runner against the deployed API.
4. Revokes membership/ACL and measures commit-to-deny, requiring zero stale authorization and ≤3000 ms.
5. Injects quota crash/retry/cancel and requires zero drift after reconcile.
6. Runs noisy-org ingest while measuring quiet-org query P95 and starvation.
7. Queries `pg_roles`/`current_user` through the worker path and proves `markhand_worker`, non-superuser, non-BYPASSRLS.
8. Maps token rotation/reuse/revoke, Qdrant timeout/partial failure, and reconcile scope to evidence.
9. Runs the vulnerability scan and requires dispositions.
10. Redaction-scans every output before writing final JSON.

Resolve the latest stable Trivy release during implementation, resolve its immutable image digest, and commit only `version@sha256`; never use `latest`, `curl | bash`, or an unpinned action.

- [ ] **Step 4: Commit/push and run hermetic GREEN**

```bash
git add deploy/scripts/phase1c-multi-org-seed.sh \
  deploy/scripts/g1c-security-gate.sh \
  bench/markhand_web/scripts/run_phase1c_gate.py \
  bench/markhand_web/scripts/test_run_phase1c_gate.py \
  crates/server/tests/e2e_phase1c_gate.rs \
  deploy/compose.poc.yml deploy/.env.example \
  deploy/poc/images.lock.json \
  docs/runbooks/phase-1c/g1c-security-gate.md
git commit -m "feat(gate): add deployed Phase 1C qualification harness"
git push -u origin cursor/phase1c-security-gate-6ddb
python3 bench/markhand_web/scripts/test_run_phase1c_gate.py
python3 scripts/check-markhand-gates.py
bash deploy/scripts/poc-isolation-smoke.sh
```

Composer reviews report integrity, scanner pinning, secret handling, and worker proof.

---

### Task 17: Add opt-in CI, run qualification, and close Phase 1C

**Files:**
- Modify: `.github/workflows/ci.yml`
- Create/generated: `bench/markhand_web/reports/phase-1c-gate/phase-1c-gate.json`
- Create: `plans/reports/phase-1c-gate-final.md`
- Modify: `docs/markhand-web-risk-register.md`
- Modify: all Phase 1C status/roadmap/generated issue artifacts

**Interfaces:**
- Adds opt-in job `phase1c-g1c-security-gate`, triggered by workflow dispatch or label `run-phase1c-gate`.
- Produces the deployed half of 1C-08/1C-12 and all 1C-13 evidence.

- [ ] **Step 1: Add the opt-in workflow**

Mirror teardown/artifact behavior from `phase1b-o04-release-gate`. The job must:

```text
install native dependencies
load required AppArmor profile
boot POC with mock embedding and dedicated worker URL
seed two orgs
run g1c-security-gate.sh
upload all sanitized evidence even on failure
dump service logs only on failure
always docker compose down -v
```

- [ ] **Step 2: Commit/push and request qualifying run**

```bash
git add .github/workflows/ci.yml
git commit -m "ci(gate): run opt-in Phase 1C security qualification"
git push -u origin cursor/phase1c-security-gate-6ddb
```

Trigger with workflow dispatch or apply `run-phase1c-gate`. If the GitHub runner cannot match the approved profile, run the same harness on the approved owner Docker host; do not change `targetMatch` manually.

- [ ] **Step 3: Validate qualifying evidence**

```bash
python3 scripts/check-markhand-gates.py
python3 bench/markhand_web/scripts/run_phase1c_gate.py \
  --validate-report bench/markhand_web/reports/phase-1c-gate/phase-1c-gate.json
MARKHAND_PHASE1C_GATE=1 \
MARKHAND_PHASE1C_REPORT_PATH=bench/markhand_web/reports/phase-1c-gate/phase-1c-gate.json \
cargo test -p fileconv-server --test e2e_phase1c_gate -- --ignored --nocapture
```

Expected:

```text
status = pass
targetMatch = true
leakageCount = 0
all G1C-* passed
worker runtime role = markhand_worker
redactionScan.passed = true
undispositioned high/critical = 0
```

- [ ] **Step 4: Record final status and regenerate**

Mark:

```text
1C-08 Done
1C-12 Done
1C-13 Done
Phase 1C = 13/13 Done
```

Update the risk register only with measured evidence; do not close the production scale risk unless the approved `on-prem-reference` profile was actually measured.

```bash
python3 scripts/build-roadmap.py
python3 scripts/build-roadmap.py --check
python3 scripts/sync-github-issues.py \
  --export-json plans/markhand-web/backlog/github-issues.json
python3 scripts/sync-github-issues.py --dry-run
```

- [ ] **Step 5: Commit/push and final independent review**

```bash
git add bench/markhand_web/reports/phase-1c-gate \
  plans/reports docs/markhand-web-risk-register.md \
  plans/markhand-web/backlog/phase-1c/issues/README.md \
  plans/markhand-web/README.md docs/project-roadmap.md \
  plans/markhand-web/roadmap.html \
  plans/markhand-web/backlog/github-issues.json
git commit -m "docs(web): close Phase 1C with deployed gate evidence"
git push -u origin cursor/phase1c-security-gate-6ddb
```

Composer reviews the final diff and report; coordinator runs the full required preflight, updates the draft PR, and waits for CI. Do not merge or mark ready without explicit owner instruction.

---

## Final Verification Matrix

| Requirement | Evidence |
|---|---|
| Canonical RBAC | Fixture invariants + OpenAPI/web parity + DB matrix |
| Organization/membership | `orgs`, `members`, last-owner/invite/session integration |
| Group/role ACL | resolver/SQL equivalence + grant/revoke/cache tests |
| No dormant grants | migration preflight + trigger shape + concurrent race test |
| Dual route/service guards | guard inventory + direct-service denial + HTTP allow/deny |
| Worker least privilege | CI role test + deployed runtime-role proof |
| Quota/fairness | 100-reservation tests + deployed recovery/noisy-neighbor metrics |
| Audit | mutation coverage ratio 1.0 + audit read/redaction + accepted retention risk |
| Multi-org denial | connected manifest, required mode, zero leakage in CI and deployment |
| Supply chain | pinned scanner + zero undispositioned high/critical |
| Phase gate | `targetMatch=true`, all `G1C-*` pass, final report sanitized |

## Execution Handoff

This plan is intended for subagent-driven execution: one fresh implementer per task, the assigned independent reviewer after each task, and a whole-PR review before CI evidence/status updates. Start only after this design/plan PR merges to `master`; create PR 1 from that updated `master`.
