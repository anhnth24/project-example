# Database ERD Design

## Goal

Create two readable JPEG entity-relationship diagrams that document the project's
active relational databases:

1. The Markhand Web PostgreSQL schema.
2. The Markhand Desktop SQLite knowledge-index schema.

The diagrams must reflect the final schema after all current migrations rather
than Rust structs, test fixtures, or planned scaffolding.

## Source of truth

- PostgreSQL: `crates/server/migrations/*.sql` and the migration ledger defined
  in `crates/server/src/database.rs`.
- SQLite: schema creation and additive upgrades in
  `crates/knowledge/src/desktop/sqlite.rs`.
- Rust database models are used only to clarify intent and logical links.

Qdrant, MinIO, and the HNSW files are external stores or caches, not relational
tables, so they will not be rendered as ERD entities.

## Deliverables

- `docs/erd/postgresql-erd.jpg`
- `docs/erd/sqlite-erd.jpg`
- Graphviz source files beside the images so future schema changes can be
  rerendered without manually redrawing the diagrams.

## PostgreSQL diagram

The PostgreSQL ERD will contain every active table, including
`markhand_schema_migrations` and `ops_fences`. Tables will be grouped by color
into identity/RBAC, collection ACL, documents, indexing/retrieval, jobs/events,
quota/audit, uploads/streaming, and infrastructure.

Each entity will show a concise Vietnamese description of the table's purpose,
its columns, SQL types, primary keys, foreign keys, and nullable fields. Solid
relationship lines represent database-enforced foreign keys. Important
self-references and composite foreign keys are included. Application-level UUID
references without foreign-key constraints are not drawn as enforced
relationships.

The image uses a high-resolution landscape layout because the complete schema is
too large for a conventional page-sized diagram.

### PostgreSQL table purposes

The purpose row in each entity uses the following explicit inventory:

- Identity and RBAC: `orgs` stores tenants; `users` stores global user accounts;
  `org_memberships` assigns users and basic roles to tenants; `permissions`
  defines canonical actions; `roles` defines tenant RBAC roles;
  `role_permissions` maps permissions to roles; `groups` defines tenant user
  groups; `group_memberships` maps users to groups; `refresh_tokens` stores
  refresh-token sessions and rotation lineage; and `org_invites` stores pending
  tenant invitations.
- Collection ACL: `collections` stores document libraries and their sharing
  scope; `collection_user_access`, `collection_group_access`, and
  `collection_role_access` grant collection access to a user, group, or role,
  respectively.
- Documents: `documents` stores logical documents and their current-version
  pointer; `document_versions` stores immutable content snapshots and publication
  lineage; and `derived_artifacts` stores immutable outputs generated from a
  document version.
- Indexing and retrieval: `index_metadata` identifies retrieval configurations
  and index generations; `chunks` stores searchable, citable text units;
  `claims` stores typed facts extracted from versions and chunks; `conflicts`
  records detected claim conflicts; `conflict_evidence` stores immutable evidence
  for those conflicts; `index_generation_backfills` tracks version backfill into
  an index generation; `embedding_batches` records durable embedding work; and
  `vector_cleanup_intents` coordinates recoverable Qdrant vector cleanup.
- Jobs and events: `jobs` is the durable worker queue; `outbox_events` is the
  transactional event outbox; and `event_log` is the tenant-sequenced event
  history.
- Quota and audit: `org_quotas` stores tenant resource limits;
  `usage_counters` stores period usage; `quota_reservations` reserves capacity
  for in-flight work; and `audit_log` is the append-only security and operations
  audit trail.
- Uploads and streaming: `upload_operations` stores upload idempotency and
  MinIO/database reconciliation state; `download_capability_redemptions` records
  single-use download capabilities; `ask_stream_sessions` stores resumable ask
  SSE sessions and pinned snapshots; and `ask_stream_events` stores each
  session's ordered SSE events.
- Infrastructure: `ops_fences` stores global restore/reconcile safety fences,
  and `markhand_schema_migrations` records the checksum and application time of
  each immutable migration.

Columns such as `upload_operations.collection_id`, `document_id`, `version_id`,
and `job_id`, plus UUID arrays in `ask_stream_sessions`, are application-level
references because the migrations do not declare foreign keys for them. They are
marked `REF` and connected with visually distinct dashed lines whose labels state
that the relationship is maintained by the application. They are never marked
`FK` or connected by a solid line.

## SQLite diagram

The SQLite ERD will show `documents`, `chunks`, `index_meta`, and the
`chunks_fts` FTS5 virtual table, with a concise Vietnamese purpose description
for each entity. The `documents` to `chunks` association and the `chunks` to
`chunks_fts` mirror are marked as logical/application-maintained relationships
because SQLite does not declare foreign keys for them.

## Visual conventions

- Entity header: table name.
- Column markers: `PK`, `FK`, `REF`, or combinations where applicable.
- A trailing `?` marks a nullable column.
- Crow's-foot endpoints distinguish exactly one, zero-or-one, and zero-or-many.
- Every composite FK has a visible complete local-tuple → referenced-tuple label.
- A legend distinguishes enforced and logical relationships.
- JPEG output uses a white background and sufficient resolution for zooming.

## Validation

- Compare the entity list against all migration `CREATE TABLE` statements and
  later `ALTER TABLE` additions.
- Run `python3 docs/erd/validate_erd.py` to compare entities, final columns,
  nullability, FK tuples/cardinality, and logical links against source.
- Render Graphviz sources and verify both JPEG files are valid, non-empty images
  with legible dimensions.
- Review the generated images for clipping, overlapping entities, and unreadable
  labels.
