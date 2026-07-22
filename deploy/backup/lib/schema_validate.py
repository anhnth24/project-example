#!/usr/bin/env python3
"""Minimal JSON Schema subset validator for recovery manifests (stdlib only)."""

from __future__ import annotations

import copy
import json
import re
from pathlib import Path
from typing import Any


class SchemaError(ValueError):
    pass


SCHEMA_PATH = (
    Path(__file__).resolve().parents[1] / "schema" / "recovery-manifest.schema.json"
)


def load_schema(path: Path | None = None) -> dict[str, Any]:
    schema_path = path or SCHEMA_PATH
    data = json.loads(schema_path.read_text(encoding="utf-8"))
    if not isinstance(data, dict):
        raise SchemaError("schema root must be object")
    return data


def resolve_ref(schema: dict[str, Any], root: dict[str, Any]) -> dict[str, Any]:
    ref = schema.get("$ref")
    if not isinstance(ref, str):
        return schema
    if not ref.startswith("#/"):
        raise SchemaError(f"unsupported $ref: {ref}")
    node: Any = root
    for part in ref[2:].split("/"):
        part = part.replace("~1", "/").replace("~0", "~")
        if not isinstance(node, dict) or part not in node:
            raise SchemaError(f"unresolved $ref: {ref}")
        node = node[part]
    if not isinstance(node, dict):
        raise SchemaError(f"$ref target must be object: {ref}")
    merged = copy.deepcopy(node)
    for key, value in schema.items():
        if key == "$ref":
            continue
        merged[key] = value
    return merged


def validate(
    instance: Any,
    schema: dict[str, Any],
    *,
    root: dict[str, Any] | None = None,
    path: str = "$",
) -> None:
    root = root if root is not None else schema
    schema = resolve_ref(schema, root)
    if "const" in schema and instance != schema["const"]:
        raise SchemaError(f"{path}: expected const {schema['const']!r}")
    if "enum" in schema and instance not in schema["enum"]:
        raise SchemaError(f"{path}: value {instance!r} not in enum")
    expected_type = schema.get("type")
    if expected_type is not None:
        _check_type(instance, expected_type, path)
    if isinstance(instance, dict):
        if "minProperties" in schema and len(instance) < int(schema["minProperties"]):
            raise SchemaError(f"{path}: fewer than minProperties")
        if "maxProperties" in schema and len(instance) > int(schema["maxProperties"]):
            raise SchemaError(f"{path}: more than maxProperties")
        _validate_object(instance, schema, root, path)
    elif isinstance(instance, list):
        _validate_array(instance, schema, root, path)
    elif isinstance(instance, str):
        if "minLength" in schema and len(instance) < int(schema["minLength"]):
            raise SchemaError(f"{path}: string shorter than minLength")
        if "maxLength" in schema and len(instance) > int(schema["maxLength"]):
            raise SchemaError(f"{path}: string longer than maxLength")
        if "pattern" in schema and re.fullmatch(str(schema["pattern"]), instance) is None:
            raise SchemaError(f"{path}: string does not match pattern")
    elif isinstance(instance, bool):
        pass
    elif isinstance(instance, (int, float)) and not isinstance(instance, bool):
        if "minimum" in schema and instance < schema["minimum"]:
            raise SchemaError(f"{path}: below minimum")
        if "maximum" in schema and instance > schema["maximum"]:
            raise SchemaError(f"{path}: above maximum")
    if "oneOf" in schema:
        matches = 0
        errors: list[str] = []
        for idx, sub in enumerate(schema["oneOf"]):
            try:
                validate(instance, sub, root=root, path=f"{path}/oneOf/{idx}")
                matches += 1
            except SchemaError as exc:
                errors.append(str(exc))
        if matches != 1:
            raise SchemaError(f"{path}: oneOf matched {matches} schemas; errors={errors[:3]}")


def validate_manifest(instance: Any, schema: dict[str, Any] | None = None) -> None:
    root = schema if schema is not None else load_schema()
    validate(instance, root, root=root, path="$")


def _check_type(instance: Any, expected: str | list[str], path: str) -> None:
    types = expected if isinstance(expected, list) else [expected]
    for t in types:
        if t == "object" and isinstance(instance, dict):
            return
        if t == "array" and isinstance(instance, list):
            return
        if t == "string" and isinstance(instance, str):
            return
        if t == "boolean" and isinstance(instance, bool):
            return
        if t == "integer" and isinstance(instance, int) and not isinstance(instance, bool):
            return
        if t == "number" and isinstance(instance, (int, float)) and not isinstance(instance, bool):
            return
        if t == "null" and instance is None:
            return
    raise SchemaError(f"{path}: expected type {expected}, got {type(instance).__name__}")


def _validate_object(
    instance: dict[str, Any],
    schema: dict[str, Any],
    root: dict[str, Any],
    path: str,
) -> None:
    props = schema.get("properties", {})
    required = schema.get("required", [])
    for key in required:
        if key not in instance:
            raise SchemaError(f"{path}: missing required property {key!r}")
    additional = schema.get("additionalProperties", True)
    pattern_props = schema.get("patternProperties") or {}
    for key, value in instance.items():
        if key in props:
            validate(value, props[key], root=root, path=f"{path}.{key}")
            continue
        matched_pattern = False
        for pattern, subschema in pattern_props.items():
            if re.fullmatch(pattern, key):
                validate(value, subschema, root=root, path=f"{path}.{key}")
                matched_pattern = True
                break
        if matched_pattern:
            continue
        if additional is False:
            raise SchemaError(f"{path}: unknown property {key!r}")
        if isinstance(additional, dict):
            validate(value, additional, root=root, path=f"{path}.{key}")


def _validate_array(
    instance: list[Any],
    schema: dict[str, Any],
    root: dict[str, Any],
    path: str,
) -> None:
    if "minItems" in schema and len(instance) < int(schema["minItems"]):
        raise SchemaError(f"{path}: fewer than minItems")
    if "maxItems" in schema and len(instance) > int(schema["maxItems"]):
        raise SchemaError(f"{path}: more than maxItems")
    items = schema.get("items")
    if isinstance(items, dict):
        for idx, value in enumerate(instance):
            validate(value, items, root=root, path=f"{path}[{idx}]")
