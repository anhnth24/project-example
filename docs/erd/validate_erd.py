#!/usr/bin/env python3
"""Validate the checked-in ERDs against their authoritative database sources.

The PostgreSQL inventory is reconstructed by applying table/column/constraint
DDL from the immutable migrations and the migration-ledger DDL in database.rs.
The SQLite inventory comes from the DDL and additive upgrades in sqlite.rs.
Logical links are a small semantic catalog backed by concrete Rust fields in the
database adapters; they are deliberately not counted as foreign keys.
"""

from __future__ import annotations

import argparse
from collections import OrderedDict
from dataclasses import dataclass, field
import html
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
from typing import Iterable


@dataclass
class Column:
    name: str
    sql_type: str
    nullable: bool
    declared_not_null: bool = False
    primary_key: bool = False


@dataclass
class Table:
    name: str
    columns: OrderedDict[str, Column] = field(default_factory=OrderedDict)
    primary_key: tuple[str, ...] = ()
    unique_keys: set[tuple[str, ...]] = field(default_factory=set)
    named_unique_keys: dict[str, tuple[str, ...]] = field(default_factory=dict)


@dataclass(frozen=True)
class ForeignKey:
    child: str
    local: tuple[str, ...]
    parent: str
    remote: tuple[str, ...]
    name: str = ""

    @property
    def key(self) -> tuple[str, tuple[str, ...], str, tuple[str, ...]]:
        return (self.child, self.local, self.parent, self.remote)


@dataclass
class Schema:
    tables: OrderedDict[str, Table] = field(default_factory=OrderedDict)
    foreign_keys: list[ForeignKey] = field(default_factory=list)


@dataclass(frozen=True)
class LogicalLink:
    source_table: str
    source_column: str
    target_table: str
    target_column: str
    array: bool
    evidence_file: str = ""
    evidence_pattern: str = ""

    @property
    def key(self) -> tuple[str, str, str, str]:
        return (
            self.source_table,
            self.source_column,
            self.target_table,
            self.target_column,
        )


@dataclass
class DotColumn:
    marker: str
    name: str
    type_text: str


@dataclass
class DotNode:
    name: str
    purpose: str
    columns: list[DotColumn]


@dataclass
class DotEdge:
    tail_table: str
    tail_port: str
    head_table: str
    head_port: str
    attrs: dict[str, str]


SQLITE_LOGICAL_LINKS = (
    LogicalLink(
        "chunks",
        "doc_rel",
        "documents",
        "doc_rel",
        False,
        "crates/knowledge/src/desktop/sqlite.rs",
        r"DELETE FROM chunks WHERE doc_rel = \?1",
    ),
    LogicalLink(
        "chunks_fts",
        "chunk_id",
        "chunks",
        "chunk_id",
        False,
        "crates/knowledge/src/desktop/sqlite.rs",
        r"INSERT INTO chunks_fts \(chunk_id, doc_rel, heading, body, folded\)",
    ),
)

VIETNAMESE_RE = re.compile(
    r"[ăâđêôơưĂÂĐÊÔƠƯáàảãạấầẩẫậắằẳẵặéèẻẽẹếềểễệ"
    r"íìỉĩịóòỏõọốồổỗộớờởỡợúùủũụứừửữựýỳỷỹỵ]"
)


def strip_sql_comments(source: str) -> str:
    source = re.sub(r"/\*.*?\*/", " ", source, flags=re.S)
    return re.sub(r"--[^\n]*", " ", source)


def split_top_level(text: str, separator: str = ",") -> list[str]:
    parts: list[str] = []
    start = 0
    depth = 0
    quote: str | None = None
    index = 0
    while index < len(text):
        char = text[index]
        if quote:
            if char == quote:
                if index + 1 < len(text) and text[index + 1] == quote:
                    index += 2
                    continue
                quote = None
            index += 1
            continue
        if char in ("'", '"'):
            quote = char
        elif char == "(":
            depth += 1
        elif char == ")":
            depth -= 1
        elif char == separator and depth == 0:
            parts.append(text[start:index].strip())
            start = index + 1
        index += 1
    parts.append(text[start:].strip())
    return [part for part in parts if part]


def matching_parenthesis(source: str, opening: int) -> int:
    depth = 0
    quote: str | None = None
    index = opening
    while index < len(source):
        char = source[index]
        if quote:
            if char == quote:
                if index + 1 < len(source) and source[index + 1] == quote:
                    index += 2
                    continue
                quote = None
            index += 1
            continue
        if char in ("'", '"'):
            quote = char
        elif char == "(":
            depth += 1
        elif char == ")":
            depth -= 1
            if depth == 0:
                return index
        index += 1
    raise ValueError("unclosed SQL parenthesis")


def normalize_identifier(value: str) -> str:
    return value.strip().strip('"').split(".")[-1]


def parse_column_list(value: str) -> tuple[str, ...]:
    return tuple(normalize_identifier(part) for part in split_top_level(value))


def normalize_type(value: str) -> str:
    return re.sub(r"\s+", " ", value.strip()).lower()


def constraint_prefix(item: str) -> tuple[str, str]:
    match = re.match(
        r"CONSTRAINT\s+(?:\"([^\"]+)\"|([A-Za-z_]\w*))\s+(.*)",
        item,
        flags=re.I | re.S,
    )
    if not match:
        return "", item.strip()
    return match.group(1) or match.group(2), match.group(3).strip()


def parse_foreign_key(
    child: str, item: str, name: str = ""
) -> ForeignKey | None:
    match = re.search(
        r"FOREIGN\s+KEY\s*\((.*?)\)\s*"
        r"REFERENCES\s+([A-Za-z_][\w.\"]*)\s*\((.*?)\)",
        item,
        flags=re.I | re.S,
    )
    if not match:
        return None
    return ForeignKey(
        child=child,
        local=parse_column_list(match.group(1)),
        parent=normalize_identifier(match.group(2)),
        remote=parse_column_list(match.group(3)),
        name=name,
    )


def add_table_item(
    schema: Schema, table: Table, item: str, *, postgres: bool
) -> None:
    constraint_name, definition = constraint_prefix(item)
    upper = definition.upper()
    if upper.startswith("PRIMARY KEY"):
        match = re.search(r"PRIMARY\s+KEY\s*\((.*?)\)", definition, re.I | re.S)
        if match:
            table.primary_key = parse_column_list(match.group(1))
        return
    if upper.startswith("UNIQUE"):
        match = re.search(r"UNIQUE\s*\((.*?)\)", definition, re.I | re.S)
        if match:
            columns = parse_column_list(match.group(1))
            table.unique_keys.add(columns)
            if constraint_name:
                table.named_unique_keys[constraint_name] = columns
        return
    if upper.startswith("FOREIGN KEY"):
        foreign_key = parse_foreign_key(table.name, definition, constraint_name)
        if foreign_key:
            schema.foreign_keys.append(foreign_key)
        return
    if re.match(r"^(?:CHECK|EXCLUDE)\b", upper):
        return

    column_match = re.match(r'(?:\"([^\"]+)\"|([A-Za-z_]\w*))\s+(.*)', item, re.S)
    if not column_match:
        return
    column_name = column_match.group(1) or column_match.group(2)
    remainder = column_match.group(3).strip()
    keyword = re.search(
        r"\s+(?:NOT\s+NULL|NULL|DEFAULT|PRIMARY\s+KEY|UNIQUE|REFERENCES|"
        r"CHECK|COLLATE|GENERATED)\b",
        " " + remainder,
        flags=re.I,
    )
    type_end = keyword.start() if keyword else len(" " + remainder)
    sql_type = normalize_type((" " + remainder)[:type_end])
    declared_not_null = bool(re.search(r"\bNOT\s+NULL\b", remainder, re.I))
    inline_primary = bool(re.search(r"\bPRIMARY\s+KEY\b", remainder, re.I))
    nullable = not declared_not_null
    if postgres and inline_primary:
        nullable = False
    column = Column(
        name=column_name,
        sql_type=sql_type,
        nullable=nullable,
        declared_not_null=declared_not_null,
        primary_key=inline_primary,
    )
    table.columns[column_name] = column
    if inline_primary:
        table.primary_key = (column_name,)
    if re.search(r"\bUNIQUE\b", remainder, re.I):
        table.unique_keys.add((column_name,))
    reference = re.search(
        r"\bREFERENCES\s+([A-Za-z_][\w.\"]*)\s*\((.*?)\)",
        remainder,
        re.I | re.S,
    )
    if reference:
        schema.foreign_keys.append(
            ForeignKey(
                child=table.name,
                local=(column_name,),
                parent=normalize_identifier(reference.group(1)),
                remote=parse_column_list(reference.group(2)),
            )
        )


def scan_create_tables(
    schema: Schema, source: str, *, postgres: bool
) -> None:
    clean = strip_sql_comments(source)
    pattern = re.compile(
        r"\bCREATE\s+TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?"
        r"(?:\"([^\"]+)\"|([A-Za-z_]\w*))\s*\(",
        re.I,
    )
    for match in pattern.finditer(clean):
        name = match.group(1) or match.group(2)
        opening = clean.find("(", match.start())
        closing = matching_parenthesis(clean, opening)
        table = schema.tables.setdefault(name, Table(name))
        for item in split_top_level(clean[opening + 1 : closing]):
            add_table_item(schema, table, item, postgres=postgres)


def add_column_from_action(table: Table, action: str, *, postgres: bool) -> None:
    match = re.match(
        r"ADD\s+COLUMN\s+(?:IF\s+NOT\s+EXISTS\s+)?(.*)",
        action,
        flags=re.I | re.S,
    )
    if not match:
        return
    temporary = Schema(OrderedDict([(table.name, table)]))
    add_table_item(temporary, table, match.group(1), postgres=postgres)


def apply_alter_tables(schema: Schema, source: str, *, postgres: bool) -> None:
    clean = strip_sql_comments(source)
    for match in re.finditer(
        r"\bALTER\s+TABLE\s+(?:IF\s+EXISTS\s+)?"
        r"(?:\"([^\"]+)\"|([A-Za-z_]\w*))\s+(.*?);",
        clean,
        flags=re.I | re.S,
    ):
        table_name = match.group(1) or match.group(2)
        table = schema.tables.get(table_name)
        if table is None:
            continue
        for action in split_top_level(match.group(3)):
            upper = action.upper()
            if upper.startswith("ADD COLUMN"):
                add_column_from_action(table, action, postgres=postgres)
                continue
            if upper.startswith("ADD CONSTRAINT"):
                name, definition = constraint_prefix(
                    re.sub(r"^ADD\s+", "", action, flags=re.I)
                )
                if definition.upper().startswith("FOREIGN KEY"):
                    foreign_key = parse_foreign_key(table_name, definition, name)
                    if foreign_key:
                        schema.foreign_keys.append(foreign_key)
                elif definition.upper().startswith("UNIQUE"):
                    unique = re.search(
                        r"UNIQUE\s*\((.*?)\)", definition, re.I | re.S
                    )
                    if unique:
                        columns = parse_column_list(unique.group(1))
                        table.unique_keys.add(columns)
                        if name:
                            table.named_unique_keys[name] = columns
                continue
            dropped = re.match(
                r"DROP\s+CONSTRAINT\s+(?:IF\s+EXISTS\s+)?"
                r"(?:\"([^\"]+)\"|([A-Za-z_]\w*))",
                action,
                flags=re.I,
            )
            if dropped:
                name = dropped.group(1) or dropped.group(2)
                columns = table.named_unique_keys.pop(name, None)
                if columns:
                    table.unique_keys.discard(columns)


def apply_unique_indexes(schema: Schema, source: str) -> None:
    clean = strip_sql_comments(source)
    for match in re.finditer(
        r"\bCREATE\s+UNIQUE\s+INDEX\s+(?:IF\s+NOT\s+EXISTS\s+)?"
        r"[A-Za-z_]\w*\s+ON\s+([A-Za-z_]\w*)\s*\(",
        clean,
        flags=re.I,
    ):
        opening = clean.find("(", match.start())
        closing = matching_parenthesis(clean, opening)
        suffix = clean[closing + 1 : clean.find(";", closing) + 1]
        if re.search(r"\bWHERE\b", suffix, re.I):
            continue
        body = clean[opening + 1 : closing]
        columns = parse_column_list(body)
        if all(re.fullmatch(r"[A-Za-z_]\w*", column) for column in columns):
            table = schema.tables.get(match.group(1))
            if table:
                table.unique_keys.add(columns)


def finalize_schema(schema: Schema, *, postgres: bool) -> Schema:
    for table in schema.tables.values():
        if table.primary_key:
            table.unique_keys.add(table.primary_key)
        for column_name in table.primary_key:
            column = table.columns.get(column_name)
            if column:
                column.primary_key = True
                if postgres:
                    column.nullable = False
    return schema


def parse_postgresql_sources(sources: Iterable[str]) -> Schema:
    sources = list(sources)
    schema = Schema()
    for source in sources:
        scan_create_tables(schema, source, postgres=True)
    for source in sources:
        apply_alter_tables(schema, source, postgres=True)
        apply_unique_indexes(schema, source)
    return finalize_schema(schema, postgres=True)


def parse_sqlite_source(source: str) -> Schema:
    schema = Schema()
    scan_create_tables(schema, source, postgres=False)
    fts_pattern = re.compile(
        r"\bCREATE\s+VIRTUAL\s+TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?"
        r"([A-Za-z_]\w*)\s+USING\s+fts5\s*\(",
        re.I,
    )
    for match in fts_pattern.finditer(source):
        opening = source.find("(", match.start())
        closing = matching_parenthesis(source, opening)
        table = Table(match.group(1))
        for item in split_top_level(source[opening + 1 : closing]):
            if item.lower().startswith("tokenize="):
                continue
            column_name = item.split()[0]
            table.columns[column_name] = Column(column_name, "—", True)
        schema.tables[table.name] = table
    for match in re.finditer(
        r"ensure_column\(\s*&connection,\s*\"([^\"]+)\",\s*"
        r"\"([^\"]+)\",\s*\"([^\"]+)\"",
        source,
        flags=re.S,
    ):
        table_name, column_name, definition = match.groups()
        table = schema.tables.get(table_name)
        if table and column_name not in table.columns:
            add_column_from_action(
                table,
                f"ADD COLUMN {column_name} {definition}",
                postgres=False,
            )
    return finalize_schema(schema, postgres=False)


def classify_cardinality(schema: Schema, foreign_key: ForeignKey) -> tuple[str, str]:
    child = schema.tables[foreign_key.child]
    parent_side = (
        "0..1"
        if any(child.columns[column].nullable for column in foreign_key.local)
        else "1"
    )
    local_set = set(foreign_key.local)
    unique = any(set(key).issubset(local_set) for key in child.unique_keys)
    child_side = "0..1" if unique else "0..many"
    return parent_side, child_side


def derive_postgresql_logical_links(schema: Schema) -> tuple[LogicalLink, ...]:
    """Infer app references from unconstrained conventional UUID field names."""
    targets = {
        "collection_id": ("collections", "id"),
        "collection_ids": ("collections", "id"),
        "document_id": ("documents", "id"),
        "document_ids": ("documents", "id"),
        "version_id": ("document_versions", "id"),
        "version_ids": ("document_versions", "id"),
        "job_id": ("jobs", "id"),
        "job_ids": ("jobs", "id"),
    }
    enforced_columns: dict[str, set[str]] = {}
    for foreign_key in schema.foreign_keys:
        enforced_columns.setdefault(foreign_key.child, set()).update(
            foreign_key.local
        )
    links: list[LogicalLink] = []
    for table in schema.tables.values():
        for column in table.columns.values():
            if column.name in enforced_columns.get(table.name, set()):
                continue
            conventional_name = re.sub(r"^cited_", "", column.name)
            target = targets.get(conventional_name)
            if target is None or column.sql_type not in {"uuid", "uuid[]"}:
                continue
            target_table, target_column = target
            if target_table not in schema.tables:
                continue
            links.append(
                LogicalLink(
                    source_table=table.name,
                    source_column=column.name,
                    target_table=target_table,
                    target_column=target_column,
                    array=column.sql_type.endswith("[]"),
                )
            )
    return tuple(links)


def strip_tags(value: str) -> str:
    return html.unescape(re.sub(r"<[^>]+>", "", value)).strip()


def parse_dot_nodes(source: str) -> dict[str, DotNode]:
    nodes: dict[str, DotNode] = {}
    pattern = re.compile(
        r"^\s*([A-Za-z_]\w*)\s+\[label=<\s*(.*?)^\s*>\];",
        re.M | re.S,
    )
    row_pattern = re.compile(
        r"<TR><TD(?P<marker_attrs>[^>]*)>(?P<marker>.*?)</TD>"
        r"<TD(?P<name_attrs>[^>]*)>(?P<name>[^<]+)</TD>"
        r"<TD(?P<type_attrs>[^>]*)>(?P<type>[^<]+)</TD></TR>",
        re.S,
    )
    for match in pattern.finditer(source):
        name, body = match.groups()
        purpose_match = re.search(
            r"<TR><TD[^>]*COLSPAN=\"3\"[^>]*><I>(.*?)</I></TD></TR>",
            body,
            re.S,
        )
        columns = [
            DotColumn(
                marker=strip_tags(row.group("marker")),
                name=html.unescape(row.group("name")).strip(),
                type_text=html.unescape(row.group("type")).strip(),
            )
            for row in row_pattern.finditer(body)
        ]
        nodes[name] = DotNode(
            name=name,
            purpose=strip_tags(purpose_match.group(1)) if purpose_match else "",
            columns=columns,
        )
    return nodes


def parse_attrs(value: str) -> dict[str, str]:
    attrs: dict[str, str] = {}
    for match in re.finditer(
        r"([A-Za-z_]\w*)\s*=\s*(?:\"([^\"]*)\"|([A-Za-z_][\w.]*))",
        value,
    ):
        attrs[match.group(1)] = (
            html.unescape(match.group(2))
            if match.group(2) is not None
            else match.group(3)
        )
    return attrs


def parse_dot_edges(source: str) -> list[DotEdge]:
    edges: list[DotEdge] = []
    pattern = re.compile(
        r"^\s*([A-Za-z_]\w*)(?::([A-Za-z_]\w*))?(?::[a-z]+)?\s*->\s*"
        r"([A-Za-z_]\w*)(?::([A-Za-z_]\w*))?(?::[a-z]+)?\s*"
        r"(?:\[(.*?)\])?\s*;",
        re.M | re.S,
    )
    for match in pattern.finditer(source):
        edges.append(
            DotEdge(
                tail_table=match.group(1),
                tail_port=match.group(2) or "",
                head_table=match.group(3),
                head_port=match.group(4) or "",
                attrs=parse_attrs(match.group(5) or ""),
            )
        )
    return edges


def rendered_base_type(type_text: str) -> str:
    value = type_text.replace("?", "").split("·", 1)[0]
    return normalize_type(value)


def validate_nodes(
    schema: Schema,
    nodes: dict[str, DotNode],
    foreign_keys: Iterable[ForeignKey],
    logical_links: Iterable[LogicalLink],
    *,
    database: str,
) -> list[str]:
    errors: list[str] = []
    expected_tables = set(schema.tables)
    rendered_tables = set(nodes) - {"legend"}
    if rendered_tables != expected_tables:
        missing = sorted(expected_tables - rendered_tables)
        extra = sorted(rendered_tables - expected_tables)
        errors.append(f"{database}: entity drift; missing={missing}, extra={extra}")
    fk_columns: dict[str, set[str]] = {}
    for foreign_key in foreign_keys:
        fk_columns.setdefault(foreign_key.child, set()).update(foreign_key.local)
    ref_columns: dict[str, set[str]] = {}
    for link in logical_links:
        ref_columns.setdefault(link.source_table, set()).add(link.source_column)

    for table_name, table in schema.tables.items():
        node = nodes.get(table_name)
        if node is None:
            continue
        if not node.purpose or not VIETNAMESE_RE.search(node.purpose):
            errors.append(
                f"{database}: {table_name} lacks a concise Vietnamese purpose"
            )
        rendered_names = [column.name for column in node.columns]
        expected_names = list(table.columns)
        if rendered_names != expected_names:
            errors.append(
                f"{database}: {table_name} column drift; "
                f"expected={expected_names}, rendered={rendered_names}"
            )
            continue
        for rendered in node.columns:
            expected = table.columns[rendered.name]
            if rendered_base_type(rendered.type_text) != expected.sql_type:
                errors.append(
                    f"{database}: {table_name}.{rendered.name} type drift; "
                    f"expected {expected.sql_type}, rendered {rendered.type_text!r}"
                )
            rendered_nullable = "?" in rendered.type_text
            if rendered_nullable != expected.nullable:
                errors.append(
                    f"{database}: {table_name}.{rendered.name} nullability drift; "
                    f"expected nullable={expected.nullable}, "
                    f"rendered={rendered.type_text!r}"
                )
            tokens = set(re.findall(r"\b(?:PK|FK|REF)\b", rendered.marker))
            expected_tokens: set[str] = set()
            if expected.primary_key:
                expected_tokens.add("PK")
            if rendered.name in fk_columns.get(table_name, set()):
                expected_tokens.add("FK")
            if rendered.name in ref_columns.get(table_name, set()):
                expected_tokens.add("REF")
            if tokens != expected_tokens:
                errors.append(
                    f"{database}: {table_name}.{rendered.name} marker drift; "
                    f"expected={sorted(expected_tokens)}, rendered={sorted(tokens)}"
                )
    return errors


def fk_from_edge(edge: DotEdge) -> ForeignKey | None:
    attrs = edge.attrs
    if attrs.get("kind") != "fk":
        return None
    try:
        return ForeignKey(
            child=attrs["child"],
            local=parse_column_list(attrs["local_cols"]),
            parent=attrs["parent"],
            remote=parse_column_list(attrs["ref_cols"]),
            name=attrs.get("constraint", ""),
        )
    except KeyError:
        return None


def validate_fk_edges(
    schema: Schema, edges: list[DotEdge], *, database: str
) -> list[str]:
    errors: list[str] = []
    expected: dict[
        tuple[str, tuple[str, ...], str, tuple[str, ...]], list[ForeignKey]
    ] = {}
    for foreign_key in schema.foreign_keys:
        expected.setdefault(foreign_key.key, []).append(foreign_key)
    actual_edges: dict[
        tuple[str, tuple[str, ...], str, tuple[str, ...]], list[DotEdge]
    ] = {}
    malformed = [
        edge
        for edge in edges
        if edge.attrs.get("kind") == "fk" and not fk_from_edge(edge)
    ]
    for edge in malformed:
        errors.append(
            f"{database}: malformed FK metadata on "
            f"{edge.tail_table}->{edge.head_table}"
        )
    for edge in edges:
        foreign_key = fk_from_edge(edge)
        if foreign_key:
            actual_edges.setdefault(foreign_key.key, []).append(edge)
    actual_keys = set(actual_edges)
    if actual_keys != set(expected):
        missing = sorted(set(expected) - actual_keys)
        extra = sorted(actual_keys - set(expected))
        errors.append(
            f"{database}: enforced-FK drift; missing={missing}, extra={extra}"
        )
    actual_count = sum(len(matching) for matching in actual_edges.values())
    if actual_count != len(schema.foreign_keys):
        errors.append(
            f"{database}: expected {len(schema.foreign_keys)} enforced FK edges, "
            f"rendered {actual_count}"
        )
    for key, matching_edges in actual_edges.items():
        expected_fks = expected.get(key)
        if expected_fks is None:
            continue
        if len(matching_edges) != len(expected_fks):
            errors.append(
                f"{database}: FK tuple {key} represents {len(expected_fks)} "
                f"constraint(s), rendered {len(matching_edges)} time(s)"
            )
            continue
        expected_fk = expected_fks[0]
        for edge in matching_edges:
            if (
                edge.tail_table != expected_fk.parent
                or edge.head_table != expected_fk.child
            ):
                errors.append(
                    f"{database}: FK {key} must route parent→child, got "
                    f"{edge.tail_table}→{edge.head_table}"
                )
            if edge.tail_port not in expected_fk.remote:
                errors.append(
                    f"{database}: FK {key} tail port {edge.tail_port!r} is not in "
                    f"referenced tuple {expected_fk.remote}"
                )
            if edge.head_port not in expected_fk.local:
                errors.append(
                    f"{database}: FK {key} head port {edge.head_port!r} is not in "
                    f"local tuple {expected_fk.local}"
                )
            parent_cardinality, child_cardinality = classify_cardinality(
                schema, expected_fk
            )
            attrs = edge.attrs
            expected_shapes = (
                "teetee" if parent_cardinality == "1" else "teeodot",
                "teeodot" if child_cardinality == "0..1" else "crowodot",
            )
            checks = {
                "parent_cardinality": parent_cardinality,
                "child_cardinality": child_cardinality,
                "arrowtail": expected_shapes[0],
                "arrowhead": expected_shapes[1],
            }
            for attribute, expected_value in checks.items():
                if attrs.get(attribute) != expected_value:
                    errors.append(
                        f"{database}: FK {key} {attribute} must be "
                        f"{expected_value!r}, got {attrs.get(attribute)!r}"
                    )
            if len(expected_fk.local) > 1:
                expected_label = (
                    f"{expected_fk.child}({', '.join(expected_fk.local)}) → "
                    f"{expected_fk.parent}({', '.join(expected_fk.remote)})"
                )
                if attrs.get("label") != expected_label:
                    errors.append(
                        f"{database}: composite FK {key} must visibly label the "
                        f"full tuple as {expected_label!r}"
                    )
    return errors


def logical_from_edge(edge: DotEdge) -> tuple[str, str, str, str] | None:
    attrs = edge.attrs
    if attrs.get("kind") != "logical":
        return None
    required = ("source_table", "source_column", "target_table", "target_column")
    if any(name not in attrs for name in required):
        return None
    return (
        attrs["source_table"],
        attrs["source_column"],
        attrs["target_table"],
        attrs["target_column"],
    )


def validate_logical_edges(
    root: Path,
    schema: Schema,
    edges: list[DotEdge],
    links: Iterable[LogicalLink],
    *,
    database: str,
) -> list[str]:
    errors: list[str] = []
    expected = {link.key: link for link in links}
    actual: dict[tuple[str, str, str, str], list[DotEdge]] = {}
    for edge in edges:
        key = logical_from_edge(edge)
        if edge.attrs.get("kind") == "logical" and key is None:
            errors.append(
                f"{database}: malformed logical-link metadata on "
                f"{edge.tail_table}->{edge.head_table}"
            )
        elif key:
            actual.setdefault(key, []).append(edge)
    if set(actual) != set(expected):
        missing = sorted(set(expected) - set(actual))
        extra = sorted(set(actual) - set(expected))
        errors.append(
            f"{database}: logical-link drift; missing={missing}, extra={extra}"
        )
    for key, link in expected.items():
        source_table = schema.tables.get(link.source_table)
        target_table = schema.tables.get(link.target_table)
        if source_table is None or link.source_column not in source_table.columns:
            errors.append(f"{database}: logical source no longer exists: {key}")
        if target_table is None or link.target_column not in target_table.columns:
            errors.append(f"{database}: logical target no longer exists: {key}")
        if link.evidence_file:
            evidence = (root / link.evidence_file).read_text(encoding="utf-8")
            if not re.search(link.evidence_pattern, evidence, re.S):
                errors.append(
                    f"{database}: application evidence drift for {key} in "
                    f"{link.evidence_file}"
                )
        matching = actual.get(key, [])
        if len(matching) != 1:
            if matching:
                errors.append(
                    f"{database}: logical link {key} rendered {len(matching)} times"
                )
            continue
        edge = matching[0]
        attrs = edge.attrs
        if edge.tail_table != link.target_table or edge.head_table != link.source_table:
            errors.append(
                f"{database}: logical link {key} must route target→source"
            )
        if edge.tail_port != link.target_column or edge.head_port != link.source_column:
            errors.append(
                f"{database}: logical link {key} ports must be "
                f"{link.target_column}→{link.source_column}"
            )
        required_attrs = {
            "array": str(link.array).lower(),
            "app_maintained": "true",
            "arrowtail": "none",
            "arrowhead": "vee",
        }
        for attribute, expected_value in required_attrs.items():
            if attrs.get(attribute) != expected_value:
                errors.append(
                    f"{database}: logical link {key} {attribute} must be "
                    f"{expected_value!r}, got {attrs.get(attribute)!r}"
                )
        if "dashed" not in attrs.get("style", ""):
            errors.append(f"{database}: logical link {key} must be dashed")
        label = attrs.get("label", "")
        if "REF" not in label or "ứng dụng duy trì" not in label:
            errors.append(
                f"{database}: logical link {key} label must say REF and "
                "ứng dụng duy trì"
            )
        if link.array and "mảng" not in label:
            errors.append(
                f"{database}: logical array link {key} label must say mảng"
            )
    return errors


JPEG_START_OF_FRAME_MARKERS = {
    0xC0,
    0xC1,
    0xC2,
    0xC3,
    0xC5,
    0xC6,
    0xC7,
    0xC9,
    0xCA,
    0xCB,
    0xCD,
    0xCE,
    0xCF,
}


def jpeg_dimensions(path: Path) -> tuple[int, int]:
    """Return JPEG width/height using only marker metadata."""
    with path.open("rb") as image:
        if image.read(2) != b"\xff\xd8":
            raise ValueError("missing JPEG SOI marker")
        while True:
            prefix = image.read(1)
            while prefix and prefix != b"\xff":
                prefix = image.read(1)
            if not prefix:
                break
            marker_bytes = image.read(1)
            while marker_bytes == b"\xff":
                marker_bytes = image.read(1)
            if not marker_bytes:
                break
            marker = marker_bytes[0]
            if marker in {0x01, 0xD8} or 0xD0 <= marker <= 0xD7:
                continue
            if marker == 0xD9:
                break
            length_bytes = image.read(2)
            if len(length_bytes) != 2:
                raise ValueError("truncated JPEG segment length")
            length = int.from_bytes(length_bytes, "big")
            if length < 2:
                raise ValueError("invalid JPEG segment length")
            payload = image.read(length - 2)
            if len(payload) != length - 2:
                raise ValueError("truncated JPEG segment")
            if marker in JPEG_START_OF_FRAME_MARKERS:
                if len(payload) < 5:
                    raise ValueError("truncated JPEG frame header")
                height = int.from_bytes(payload[1:3], "big")
                width = int.from_bytes(payload[3:5], "big")
                if width <= 0 or height <= 0:
                    raise ValueError("JPEG has non-positive dimensions")
                return width, height
    raise ValueError("JPEG has no supported start-of-frame marker")


def render_dot(dot_path: Path, image_path: Path, *, dpi: int) -> None:
    """Render a DOT source atomically as JPEG."""
    if dpi <= 0:
        raise ValueError("DPI must be positive")
    image_path.parent.mkdir(parents=True, exist_ok=True)
    temporary_name = ""
    try:
        with tempfile.NamedTemporaryFile(
            dir=image_path.parent,
            prefix=f".{image_path.name}.",
            suffix=".tmp",
            delete=False,
        ) as temporary:
            temporary_name = temporary.name
        result = subprocess.run(
            [
                "dot",
                "-Tjpg",
                f"-Gdpi={dpi}",
                str(dot_path),
                "-o",
                temporary_name,
            ],
            check=False,
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            detail = result.stderr.strip() or result.stdout.strip()
            raise RuntimeError(f"Graphviz failed for {dot_path}: {detail}")
        jpeg_dimensions(Path(temporary_name))
        os.replace(temporary_name, image_path)
        temporary_name = ""
    except FileNotFoundError as error:
        raise RuntimeError("Graphviz 'dot' command is not installed") from error
    finally:
        if temporary_name:
            Path(temporary_name).unlink(missing_ok=True)


def render_repository_images(root: Path, *, dpi: int) -> None:
    for stem in ("postgresql-erd", "sqlite-erd"):
        render_dot(
            root / "docs/erd" / f"{stem}.dot",
            root / "docs/erd" / f"{stem}.jpg",
            dpi=dpi,
        )


def validate_graphviz_sources(root: Path) -> list[str]:
    errors: list[str] = []
    for stem in ("postgresql-erd", "sqlite-erd"):
        dot_path = root / "docs/erd" / f"{stem}.dot"
        try:
            result = subprocess.run(
                ["dot", "-Tdot", str(dot_path), "-o", os.devnull],
                check=False,
                capture_output=True,
                text=True,
            )
        except FileNotFoundError:
            return ["Graphviz: 'dot' command is not installed"]
        if result.returncode != 0:
            detail = result.stderr.strip() or result.stdout.strip()
            errors.append(f"Graphviz: invalid {dot_path.name}: {detail}")
    return errors


def validate_image_outputs(root: Path) -> list[str]:
    errors: list[str] = []
    for stem in ("postgresql-erd", "sqlite-erd"):
        image_path = root / "docs/erd" / f"{stem}.jpg"
        if not image_path.is_file():
            errors.append(f"Image: missing {image_path.relative_to(root)}")
            continue
        try:
            width, height = jpeg_dimensions(image_path)
        except (OSError, ValueError) as error:
            errors.append(
                f"Image: invalid {image_path.relative_to(root)}: {error}"
            )
            continue
        if width < 1000 or height < 600:
            errors.append(
                f"Image: {image_path.relative_to(root)} is too small "
                f"({width}x{height}; minimum 1000x600)"
            )
    return errors


def load_postgresql_schema(root: Path) -> tuple[Schema, list[str]]:
    migration_dir = root / "crates/server/migrations"
    migration_paths = sorted(migration_dir.glob("*.sql"))
    database_source = (root / "crates/server/src/database.rs").read_text(
        encoding="utf-8"
    )
    migration_block = re.search(
        r"const\s+MIGRATIONS:.*?=\s*&\[(.*?)^\];",
        database_source,
        flags=re.M | re.S,
    )
    embedded_names = (
        re.findall(r'"(\d{4}_[^"]+\.sql)"', migration_block.group(1))
        if migration_block
        else []
    )
    errors: list[str] = []
    path_names = [path.name for path in migration_paths]
    if embedded_names != path_names:
        errors.append(
            "PostgreSQL: migration ledger/file drift; "
            f"embedded={embedded_names}, files={path_names}"
        )
    sources = [path.read_text(encoding="utf-8") for path in migration_paths]
    sources.append(database_source)
    return parse_postgresql_sources(sources), errors


def validate_repository(root: Path) -> list[str]:
    errors: list[str] = []
    errors.extend(validate_graphviz_sources(root))
    postgres_schema, load_errors = load_postgresql_schema(root)
    errors.extend(load_errors)
    postgres_logical_links = derive_postgresql_logical_links(postgres_schema)
    sqlite_source = (
        root / "crates/knowledge/src/desktop/sqlite.rs"
    ).read_text(encoding="utf-8")
    sqlite_schema = parse_sqlite_source(sqlite_source)

    postgres_dot = (root / "docs/erd/postgresql-erd.dot").read_text(
        encoding="utf-8"
    )
    sqlite_dot = (root / "docs/erd/sqlite-erd.dot").read_text(encoding="utf-8")
    postgres_nodes = parse_dot_nodes(postgres_dot)
    sqlite_nodes = parse_dot_nodes(sqlite_dot)
    postgres_edges = parse_dot_edges(postgres_dot)
    sqlite_edges = parse_dot_edges(sqlite_dot)

    errors.extend(
        validate_nodes(
            postgres_schema,
            postgres_nodes,
            postgres_schema.foreign_keys,
            postgres_logical_links,
            database="PostgreSQL",
        )
    )
    errors.extend(
        validate_fk_edges(
            postgres_schema, postgres_edges, database="PostgreSQL"
        )
    )
    errors.extend(
        validate_logical_edges(
            root,
            postgres_schema,
            postgres_edges,
            postgres_logical_links,
            database="PostgreSQL",
        )
    )
    errors.extend(
        validate_nodes(
            sqlite_schema,
            sqlite_nodes,
            sqlite_schema.foreign_keys,
            SQLITE_LOGICAL_LINKS,
            database="SQLite",
        )
    )
    errors.extend(
        validate_fk_edges(sqlite_schema, sqlite_edges, database="SQLite")
    )
    errors.extend(
        validate_logical_edges(
            root,
            sqlite_schema,
            sqlite_edges,
            SQLITE_LOGICAL_LINKS,
            database="SQLite",
        )
    )
    return errors


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Validate ERD sources and checked-in JPEG outputs."
    )
    parser.add_argument(
        "--render",
        action="store_true",
        help="render both JPEGs with Graphviz before validating them",
    )
    parser.add_argument(
        "--dpi",
        type=int,
        default=180,
        help="rendering DPI used with --render (default: 180)",
    )
    args = parser.parse_args(argv)
    root = Path(__file__).resolve().parents[2]
    postgres_schema, _ = load_postgresql_schema(root)
    postgres_logical_links = derive_postgresql_logical_links(postgres_schema)
    sqlite_schema = parse_sqlite_source(
        (root / "crates/knowledge/src/desktop/sqlite.rs").read_text(
            encoding="utf-8"
        )
    )
    errors = validate_repository(root)
    if args.render and not errors:
        try:
            render_repository_images(root, dpi=args.dpi)
        except (OSError, RuntimeError, ValueError) as error:
            errors.append(f"Render: {error}")
    errors.extend(validate_image_outputs(root))
    if errors:
        print(f"ERD validation FAILED ({len(errors)} issue(s)):", file=sys.stderr)
        for error in errors:
            print(f"  - {error}", file=sys.stderr)
        return 1
    postgres_columns = sum(
        len(table.columns) for table in postgres_schema.tables.values()
    )
    postgres_composites = sum(
        len(foreign_key.local) > 1
        for foreign_key in postgres_schema.foreign_keys
    )
    sqlite_columns = sum(
        len(table.columns) for table in sqlite_schema.tables.values()
    )
    postgres_dimensions = jpeg_dimensions(root / "docs/erd/postgresql-erd.jpg")
    sqlite_dimensions = jpeg_dimensions(root / "docs/erd/sqlite-erd.jpg")
    print(
        "PostgreSQL ERD OK: "
        f"{len(postgres_schema.tables)} tables, {postgres_columns} columns, "
        f"{len(postgres_schema.foreign_keys)} enforced FKs "
        f"({postgres_composites} composite), "
        f"{len(postgres_logical_links)} logical links, "
        f"JPEG {postgres_dimensions[0]}x{postgres_dimensions[1]}"
    )
    print(
        "SQLite ERD OK: "
        f"{len(sqlite_schema.tables)} entities, {sqlite_columns} columns, "
        f"{len(sqlite_schema.foreign_keys)} enforced FKs, "
        f"{len(SQLITE_LOGICAL_LINKS)} logical links, "
        f"JPEG {sqlite_dimensions[0]}x{sqlite_dimensions[1]}"
    )
    print("ERD validation OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
