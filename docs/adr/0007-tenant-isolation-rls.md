# ADR 0007: Tenant isolation and RLS boundary

- Status: Accepted
- Date: 2026-07-18
- Decision key: `tenant-isolation-rls`
- Owners: storage-owner, security-owner, architecture-owner
- Approver: Phase 0 architecture gate
- Related issues/PRs: P0-10; ADR 0001; Phase 1B single-org POC

## Context

Markhand Web starts with a Phase 1B single-org POC, but its schema, repository
APIs and storage adapters must be multi-org-ready. ADR 0001 already requires
every business repository to receive an explicit `OrgContext`; the remaining
decision is whether tenant isolation is only an application convention or also a
database policy boundary.

The approved workload profile includes 20 orgs and hot-tenant load skew, so
missing tenant predicates are both a security bug and a performance bug. The
POC must fail closed when a query lacks tenant context without requiring real
multi-org user onboarding in Phase 1B.

## Decision

All business tables that contain user, document, collection, job, artifact,
claim, conflict, audit or quota data carry a non-null `org_id`. Repository
methods accept an `OrgContext` containing:

```text
org_id, user_id, permissions, allowed_collection_ids
```

and there is no public repository/query entry point that can read or mutate
business data without that context.

PostgreSQL Row Level Security is the accepted defense-in-depth boundary for
shared tables. The application sets tenant/session claims at transaction start,
for example:

```sql
SET LOCAL app.org_id = '<uuid>';
SET LOCAL app.user_id = '<uuid>';
```

Policies compare row `org_id` to `current_setting('app.org_id', true)` and
deny when the setting is missing. Pooled connections must clear state by using
transaction-scoped settings only; tests must prove tenant context does not leak
between pooled requests.

Qdrant and MinIO do not replace the PostgreSQL tenant boundary:

- Qdrant payload filters include mandatory `org_id` and authorized collection
  filters on every search, upsert and delete.
- MinIO object keys are opaque and include org/version identity in metadata; API
  downloads re-authorize through PostgreSQL before returning signed URLs.
- Reconciliation treats PostgreSQL as authoritative for visibility and delete
  state.

Phase 1B may seed only one org, but it must keep the `OrgContext`, RLS policy
tests and fail-closed adapter checks in place.

## Consequences

- Positive: a missing tenant predicate becomes a policy/test failure instead of
  relying on route discipline.
- Positive: the later multi-org rollout does not need a storage contract change.
- Negative: migrations and test fixtures must always include `org_id`, even for
  single-org POC data.
- Operational: connection pool setup must use transaction-scoped settings and
  avoid session-level tenant state.

## Alternatives considered

- Application-only tenant filters: rejected because one raw query or adapter
  path can bypass the convention.
- Schema-per-tenant: rejected for the POC and approved profile because it
  increases migration and connection-pool complexity before real multi-tenant
  scale is measured.
- Collection-per-org as the primary isolation boundary: rejected by ADR 0009 for
  the POC; vector topology is not the source of authorization truth.

## Verification

Phase 1B implementation must include:

```bash
python3 scripts/check-architecture-boundaries.py
cargo test -p fileconv-server tenant
```

Required test cases:

- repository calls without `OrgContext` are unavailable or fail closed;
- SQL queries under one org cannot see rows from another org;
- pooled connection reuse does not leak `app.org_id`;
- Qdrant adapter rejects search/upsert/delete without `org_id`;
- MinIO download endpoints re-authorize through PostgreSQL.

Until those live tests exist, P0-10 closes only the architecture decision. It
does not claim production multi-tenant evidence.

## Exception lifecycle

N/A. Phase 1B may be single-org in product scope, but the tenant isolation
contract is not optional in code.
