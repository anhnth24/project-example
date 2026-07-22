#!/usr/bin/env python3
"""Strict JSON loader: reject duplicates, NaN/Infinity, and unknown fields."""

from __future__ import annotations

import json
import math
import re
from typing import Any, Callable


class StrictJsonError(ValueError):
    """Fail-closed JSON error."""


_NONFINITE_TOKEN = re.compile(r"(?i)(?:^|[^A-Za-z0-9_])(NaN|-?Infinity)(?:[^A-Za-z0-9_]|$)")


def object_pairs_hook(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    out: dict[str, Any] = {}
    for key, value in pairs:
        if key in out:
            raise StrictJsonError(f"duplicate JSON key: {key}")
        out[key] = value
    return out


def _reject_nonfinite_tokens(text: str) -> None:
    if _NONFINITE_TOKEN.search(text):
        raise StrictJsonError("NaN/Infinity JSON tokens are rejected")


def reject_nonfinite(node: Any, path: str = "$") -> None:
    if isinstance(node, float):
        if math.isnan(node) or math.isinf(node):
            raise StrictJsonError(f"non-finite number at {path}")
    elif isinstance(node, dict):
        for key, value in node.items():
            reject_nonfinite(value, f"{path}.{key}")
    elif isinstance(node, list):
        for idx, value in enumerate(node):
            reject_nonfinite(value, f"{path}[{idx}]")


def loads(text: str) -> Any:
    _reject_nonfinite_tokens(text)
    try:
        data = json.loads(text, object_pairs_hook=object_pairs_hook, parse_constant=lambda s: (_ for _ in ()).throw(StrictJsonError(f"unsupported constant {s}")))
    except json.JSONDecodeError as error:
        raise StrictJsonError(str(error)) from error
    reject_nonfinite(data)
    return data


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
