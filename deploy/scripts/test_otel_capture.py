#!/usr/bin/env python3
"""Hermetic tests for the bounded O01 OTLP capture parser."""

from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).resolve().parents[1] / "poc" / "otel-capture.py"
SPEC = importlib.util.spec_from_file_location("markhand_otel_capture", MODULE_PATH)
assert SPEC and SPEC.loader
CAPTURE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CAPTURE)


class CaptureParserTests(unittest.TestCase):
    def test_normalizes_only_allowlisted_span_metadata(self) -> None:
        payload = {
            "resourceSpans": [
                {
                    "resource": {
                        "attributes": [
                            {
                                "key": "service.name",
                                "value": {"stringValue": "worker"},
                            }
                        ]
                    },
                    "scopeSpans": [
                        {
                            "spans": [
                                {
                                    "traceId": "a" * 32,
                                    "spanId": "b" * 16,
                                    "parentSpanId": "c" * 16,
                                    "name": "worker.convert",
                                    "kind": 5,
                                    "startTimeUnixNano": "1",
                                    "endTimeUnixNano": "2",
                                    "attributes": [
                                        {
                                            "key": "request_id",
                                            "value": {
                                                "stringValue": "550e8400-e29b-41d4-a716-446655440000"
                                            },
                                        },
                                        {
                                            "key": "document_body",
                                            "value": {"stringValue": "must-not-persist"},
                                        },
                                    ],
                                }
                            ]
                        }
                    ],
                }
            ]
        }
        spans = CAPTURE._normalized_spans(payload)
        self.assertEqual(len(spans), 1)
        self.assertEqual(spans[0]["name"], "worker.convert")
        self.assertEqual(spans[0]["kind"], 5)
        self.assertEqual(spans[0]["serviceName"], "worker")
        self.assertNotIn("must-not-persist", str(spans))
        self.assertNotIn("attributes", spans[0])

    def test_rejects_span_without_request_id(self) -> None:
        payload = {
            "resourceSpans": [
                {
                    "scopeSpans": [
                        {
                            "spans": [
                                {
                                    "traceId": "a" * 32,
                                    "spanId": "b" * 16,
                                    "name": "api.request",
                                    "kind": 2,
                                    "attributes": [],
                                }
                            ]
                        }
                    ]
                }
            ]
        }
        self.assertEqual(CAPTURE._normalized_spans(payload), [])


if __name__ == "__main__":
    unittest.main()
