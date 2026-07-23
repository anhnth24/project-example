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

## SQLite diagram

The SQLite ERD will show `documents`, `chunks`, `index_meta`, and the
`chunks_fts` FTS5 virtual table, with a concise Vietnamese purpose description
for each entity. The `documents` to `chunks` association and the `chunks` to
`chunks_fts` mirror are marked as logical/application-maintained relationships
because SQLite does not declare foreign keys for them.

## Visual conventions

- Entity header: table name.
- Column markers: `PK`, `FK`, or both where applicable.
- A trailing `?` marks a nullable column.
- Crow's-foot cardinality communicates one-to-many and optional links.
- A legend distinguishes enforced and logical relationships.
- JPEG output uses a white background and sufficient resolution for zooming.

## Validation

- Compare the entity list against all migration `CREATE TABLE` statements and
  later `ALTER TABLE` additions.
- Confirm every rendered foreign key against migration SQL.
- Render Graphviz sources and verify both JPEG files are valid, non-empty images
  with legible dimensions.
- Review the generated images for clipping, overlapping entities, and unreadable
  labels.
