#!/usr/bin/env python3
"""Regression tests for the repository ERD validator."""

from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))

import validate_erd


ROOT = Path(__file__).resolve().parents[2]


class ErdValidationTests(unittest.TestCase):
    def test_repository_diagrams_match_authoritative_schemas(self) -> None:
        self.assertEqual(validate_erd.validate_repository(ROOT), [])

    def test_composite_fk_cardinality_is_derived_from_schema(self) -> None:
        schema = validate_erd.parse_postgresql_sources(
            [
                """
                CREATE TABLE parent (
                    tenant_id uuid NOT NULL,
                    id uuid NOT NULL,
                    PRIMARY KEY (tenant_id, id)
                );
                CREATE TABLE many_child (
                    tenant_id uuid NOT NULL,
                    parent_id uuid NOT NULL,
                    FOREIGN KEY (tenant_id, parent_id)
                        REFERENCES parent (tenant_id, id)
                );
                CREATE TABLE optional_unique_child (
                    tenant_id uuid NOT NULL,
                    parent_id uuid,
                    UNIQUE (tenant_id, parent_id),
                    FOREIGN KEY (tenant_id, parent_id)
                        REFERENCES parent (tenant_id, id)
                );
                """
            ]
        )
        many_fk = schema.foreign_keys[0]
        optional_fk = schema.foreign_keys[1]
        self.assertEqual(
            validate_erd.classify_cardinality(schema, many_fk),
            ("1", "0..many"),
        )
        self.assertEqual(
            validate_erd.classify_cardinality(schema, optional_fk),
            ("0..1", "0..1"),
        )

    def test_postgresql_logical_links_are_derived_from_unconstrained_uuid_fields(
        self,
    ) -> None:
        schema = validate_erd.parse_postgresql_sources(
            [
                """
                CREATE TABLE collections (id uuid PRIMARY KEY);
                CREATE TABLE documents (id uuid PRIMARY KEY);
                CREATE TABLE document_versions (id uuid PRIMARY KEY);
                CREATE TABLE jobs (id uuid PRIMARY KEY);
                CREATE TABLE upload_operations (
                    collection_id uuid NOT NULL,
                    document_id uuid NOT NULL,
                    version_id uuid NOT NULL,
                    job_id uuid,
                    object_id uuid NOT NULL
                );
                CREATE TABLE ask_stream_sessions (
                    collection_ids uuid[] NOT NULL,
                    cited_document_ids uuid[] NOT NULL,
                    cited_version_ids uuid[] NOT NULL
                );
                """
            ]
        )
        links = validate_erd.derive_postgresql_logical_links(schema)
        self.assertEqual(
            {link.key for link in links},
            {
                ("upload_operations", "collection_id", "collections", "id"),
                ("upload_operations", "document_id", "documents", "id"),
                ("upload_operations", "version_id", "document_versions", "id"),
                ("upload_operations", "job_id", "jobs", "id"),
                (
                    "ask_stream_sessions",
                    "collection_ids",
                    "collections",
                    "id",
                ),
                (
                    "ask_stream_sessions",
                    "cited_document_ids",
                    "documents",
                    "id",
                ),
                (
                    "ask_stream_sessions",
                    "cited_version_ids",
                    "document_versions",
                    "id",
                ),
            },
        )

    def test_duplicate_fk_tuples_remain_distinct_constraints(self) -> None:
        schema = validate_erd.parse_postgresql_sources(
            [
                """
                CREATE TABLE parent (id uuid PRIMARY KEY);
                CREATE TABLE child (
                    parent_id uuid NOT NULL,
                    CONSTRAINT fk_child_parent_a
                        FOREIGN KEY (parent_id) REFERENCES parent (id),
                    CONSTRAINT fk_child_parent_b
                        FOREIGN KEY (parent_id) REFERENCES parent (id)
                );
                """
            ]
        )
        self.assertEqual(
            [foreign_key.name for foreign_key in schema.foreign_keys],
            ["fk_child_parent_a", "fk_child_parent_b"],
        )

    def test_jpeg_dimensions_are_read_without_third_party_packages(self) -> None:
        jpeg = (
            b"\xff\xd8"
            b"\xff\xe0\x00\x04\x00\x00"
            b"\xff\xc0\x00\x11\x08\x00\x20\x00\x30"
            b"\x03\x01\x11\x00\x02\x11\x00\x03\x11\x00"
            b"\xff\xd9"
        )
        with tempfile.TemporaryDirectory() as directory:
            image = Path(directory) / "diagram.jpg"
            image.write_bytes(jpeg)
            self.assertEqual(validate_erd.jpeg_dimensions(image), (48, 32))

    def test_graphviz_render_produces_a_valid_jpeg(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            dot_path = root / "small.dot"
            image_path = root / "small.jpg"
            dot_path.write_text(
                'digraph test { graph [bgcolor="white"]; parent -> child; }\n',
                encoding="utf-8",
            )
            validate_erd.render_dot(dot_path, image_path, dpi=24)
            width, height = validate_erd.jpeg_dimensions(image_path)
            self.assertGreater(width, 0)
            self.assertGreater(height, 0)


if __name__ == "__main__":
    unittest.main()
