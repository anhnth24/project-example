# Server migrations

Immutable expand/backfill/cutover/contract SQL for Markhand Web PostgreSQL.

## Immutability / current-published (0005)

- Content columns never UPDATE/DELETE.
- Legal publish-field transitions only (no caller-settable GUC):
  - `publication_state`: draft→published only
  - `is_current`: true→false anytime; false→true only when published
  - `effective_to`: NULL→timestamp once; never rewritten
- At-most-one current: partial unique index `uq_document_versions__document_current` (RLS-immune).
- Deferred triggers validate pointer agreement and fail closed if `app.org_id` is missing/mismatched.
- `markhand_publish_document_version()` is convenience only.

## ACL (0004)

Normalized `collection_user_access` / `collection_group_access` / `collection_role_access`
with composite FKs and `ON DELETE CASCADE` from memberships/groups/roles.

## Partition strategy (ADR 0008)

No physical partitioning for Phase 1B POC; `org_id` first in tenant indexes.

## Playbooks / Migration Guides

- [Safe backfill on tables with `FORCE ROW LEVEL SECURITY`](../../../docs/runbooks/rls-backfill-playbook.md) — Hướng dẫn backfill dữ liệu an toàn, phòng tránh lỗi SQLSTATE 42501 từ `SET row_security = off` và bẫy silent 0-row update.
