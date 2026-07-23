# Database ERD Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Produce separate, high-resolution PostgreSQL and SQLite ERD JPEGs
whose entities include concise Vietnamese purpose descriptions.

**Architecture:** Keep reviewed Graphviz DOT files as the reproducible diagram
sources and render JPEG deliverables with Graphviz. PostgreSQL entities are
clustered by subsystem; SQLite entities are rendered separately because the two
databases have independent schemas and identifiers.

**Tech Stack:** Graphviz DOT, JPEG, repository SQL migrations, Desktop SQLite
DDL.

## Global Constraints

- PostgreSQL truth comes from `crates/server/migrations/*.sql` plus the migration
  ledger in `crates/server/src/database.rs`.
- SQLite truth comes from `crates/knowledge/src/desktop/sqlite.rs`.
- Every entity must show its purpose, columns, SQL types, PK/FK markers, and
  nullability.
- Solid edges are enforced foreign keys; dashed edges are logical,
  application-maintained relationships.
- Qdrant, MinIO, and HNSW are outside the relational ERD.
- JPEGs use a white background and a resolution suitable for zooming.

---

### Task 1: PostgreSQL ERD

**Files:**
- Create: `docs/erd/postgresql-erd.dot`
- Create: `docs/erd/postgresql-erd.jpg`
- Modify: `docs/superpowers/specs/2026-07-23-database-erd-design.md`

**Interfaces:**
- Consumes: final PostgreSQL schema from migrations `0001` through `0028`.
- Produces: reproducible DOT source and the PostgreSQL JPEG deliverable.

- [ ] **Step 1: Inventory tables and foreign keys**

Run:

```bash
rg -n "CREATE TABLE|ALTER TABLE .*ADD COLUMN|FOREIGN KEY|REFERENCES" \
  crates/server/migrations crates/server/src/database.rs
```

Expected: all active PostgreSQL entities and relationship declarations are
visible for comparison with the diagram source.

- [ ] **Step 2: Create the Graphviz source**

Create an HTML-table node for every active table. Add a Vietnamese purpose row
under each table header, list final columns after all migrations, and connect
declared foreign keys using crow's-foot arrowheads. Use subsystem clusters and
distinct pastel header colors for readability.

- [ ] **Step 3: Verify entity coverage before rendering**

Run a script that extracts `CREATE TABLE` names from migrations, adds
`markhand_schema_migrations`, extracts Graphviz entity IDs, and asserts the two
sets are equal.

Expected: `PostgreSQL ERD coverage OK` and exit status 0.

- [ ] **Step 4: Render and inspect**

Run:

```bash
dot -Tjpg -Gdpi=180 docs/erd/postgresql-erd.dot \
  -o docs/erd/postgresql-erd.jpg
file docs/erd/postgresql-erd.jpg
```

Expected: a valid non-empty JPEG image. Inspect the rendered image for clipping,
overlap, and legibility.

- [ ] **Step 5: Commit**

```bash
git add docs/erd/postgresql-erd.dot docs/erd/postgresql-erd.jpg \
  docs/superpowers/specs/2026-07-23-database-erd-design.md
git commit -m "docs: add PostgreSQL database ERD"
```

### Task 2: Desktop SQLite ERD

**Files:**
- Create: `docs/erd/sqlite-erd.dot`
- Create: `docs/erd/sqlite-erd.jpg`

**Interfaces:**
- Consumes: final SQLite schema from
  `crates/knowledge/src/desktop/sqlite.rs`.
- Produces: reproducible DOT source and the SQLite JPEG deliverable.

- [ ] **Step 1: Recheck SQLite schema**

Run:

```bash
rg -n "CREATE TABLE|CREATE VIRTUAL TABLE|ensure_column|CREATE INDEX" \
  crates/knowledge/src/desktop/sqlite.rs
```

Expected: `documents`, `chunks`, `index_meta`, `chunks_fts`, additive columns,
and indexes are visible.

- [ ] **Step 2: Create the Graphviz source**

Create four entities with Vietnamese purpose descriptions. Draw dashed logical
relationships for `documents.doc_rel` to `chunks.doc_rel` and the
application-maintained `chunks` to `chunks_fts` mirror.

- [ ] **Step 3: Render and validate both deliverables**

Run:

```bash
dot -Tjpg -Gdpi=180 docs/erd/sqlite-erd.dot \
  -o docs/erd/sqlite-erd.jpg
file docs/erd/postgresql-erd.jpg docs/erd/sqlite-erd.jpg
identify docs/erd/postgresql-erd.jpg docs/erd/sqlite-erd.jpg
git diff --check
```

Expected: two valid JPEG images, positive dimensions, no whitespace errors, and
no untracked schema omissions found during manual comparison.

- [ ] **Step 4: Commit**

```bash
git add docs/erd/sqlite-erd.dot docs/erd/sqlite-erd.jpg
git commit -m "docs: add Desktop SQLite database ERD"
```
