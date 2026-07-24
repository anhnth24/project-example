#!/usr/bin/env python3
"""Hermetic tests for the POC Qdrant bootstrap helper."""

from __future__ import annotations

import importlib.util
import os
import unittest
from pathlib import Path
from unittest import mock


SCRIPT = Path(__file__).with_name("qdrant-init.py")
SPEC = importlib.util.spec_from_file_location("qdrant_init", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
qdrant_init = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(qdrant_init)


class QdrantInitTests(unittest.TestCase):
    def test_required_config_accepts_pinned_signature_and_dimensions(self) -> None:
        with mock.patch.dict(
            os.environ,
            {
                "MARKHAND_INDEX_SIGNATURE": "a" * 64,
                "MARKHAND_EMBEDDING_DIMENSIONS": "8",
                "MARKHAND_QDRANT_URL": "http://qdrant:6333/",
            },
            clear=True,
        ):
            self.assertEqual(
                qdrant_init.required_config(),
                ("a" * 64, 8, "http://qdrant:6333"),
            )

    def test_required_config_rejects_invalid_signature_and_dimensions(self) -> None:
        for signature, dimensions in (("short", "8"), ("A" * 64, "8"), ("a" * 64, "0")):
            with self.subTest(signature=signature[:8], dimensions=dimensions):
                with mock.patch.dict(
                    os.environ,
                    {
                        "MARKHAND_INDEX_SIGNATURE": signature,
                        "MARKHAND_EMBEDDING_DIMENSIONS": dimensions,
                    },
                    clear=True,
                ):
                    with self.assertRaises(SystemExit):
                        qdrant_init.required_config()

    def test_vector_config_reads_qdrant_response(self) -> None:
        payload = {
            "result": {
                "config": {
                    "params": {
                        "vectors": {
                            "size": 8,
                            "distance": "Cosine",
                        }
                    }
                }
            }
        }
        self.assertEqual(qdrant_init.vector_config(payload), (8, "cosine"))
        self.assertIsNone(qdrant_init.vector_config({"result": {}}))


if __name__ == "__main__":
    unittest.main()
