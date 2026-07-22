#!/usr/bin/env python3
"""Strict JSON loader that rejects duplicate keys and unknown fields."""

from __future__ import annotations

import json
from typing import Any, Callable


class StrictJsonError(ValueError):
    """Fail-closed JSON error."""


def object_pairs_hook(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    out: dict[str, Any] = {}
    for key, value in pairs:
        if key in out:
            raise StrictJsonError(f"duplicate JSON key: {key}")
        out[key] = value
    return out


def loads(text: str) -> Any:
    try:
        return json.loads(text, object_pairs_hook=object_pairs_hook)
    except json.JSONDecodeError as error:
        raise StrictJsonError(str(error)) from error


def load_path(path: str) -> Any:
    with open(path, encoding="utf-8") as handle:
        return loads(handle.read())


def validate_no_unknown(payload: dict[str, Any], allowed: set[str], *, where: str) -> None:
    unknown = sorted(set(payload) - allowed)
    if unknown:
        raise StrictJsonError(f"{where}: unknown fields {unknown}")


def require_fields(payload: dict[str, Any], required: set[str], *, where: str) -> None:
    missing = sorted(required - set(payload))
    if missing:
        raise StrictJsonError(f"{where}: missing fields {missing}")


def walk(
    node: Any,
    visitor: Callable[[str, Any], None],
    path: str = "$",
) -> None:
    visitor(path, node)
    if isinstance(node, dict):
        for key, value in node.items():
            walk(value, visitor, f"{path}.{key}")
    elif isinstance(node, list):
        for index, value in enumerate(node):
            walk(value, visitor, f"{path}[{index}]")
