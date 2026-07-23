# Database ERDs

The checked-in DOT files are the reproducible sources for the PostgreSQL web
schema and Desktop SQLite knowledge index. PostgreSQL truth comes from
`crates/server/migrations/*.sql` plus the migration ledger in
`crates/server/src/database.rs`; SQLite truth comes from
`crates/knowledge/src/desktop/sqlite.rs`.

## Validate

Run the source-derived validator from the repository root:

```bash
python3 docs/erd/validate_erd.py
python3 -m unittest docs/erd/test_validate_erd.py
```

It checks entities, final column order/types/nullability, PK/FK/REF markers,
all enforced FK tuples, FK cardinality, and application-maintained logical
links. PostgreSQL logical links are inferred from unconstrained conventional
UUID fields; SQLite links are backed by the corresponding adapter operations.
It also checks Graphviz syntax and verifies that both checked-in outputs are
valid JPEGs with usable dimensions. Schema, DOT, or image drift produces a
non-zero exit with field-level diagnostics.

## Render

```bash
python3 docs/erd/validate_erd.py --render
```

The default is 180 DPI; override it only for inspection renders:

```bash
python3 docs/erd/validate_erd.py --render --dpi 72
```

## Notation

- Solid subsystem-colored connectors are PostgreSQL-enforced FKs.
- Dashed red `REF` connectors are logical links maintained by the application,
  never database FKs.
- Composite FK labels show the complete local tuple and referenced tuple even
  when the connector attaches to one representative field port.
- Crow-foot endpoints use `||` for exactly one, `○|` for zero-or-one, and
  `○<` for zero-or-many. The parent-side endpoint follows local FK nullability;
  the child-side endpoint follows uniqueness of the local tuple.
- `?` means nullable.
