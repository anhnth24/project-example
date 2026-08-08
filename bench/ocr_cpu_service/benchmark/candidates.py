"""Model-neutral command candidate specifications."""

from __future__ import annotations

import math
import re
from collections.abc import Mapping as MappingABC
from collections.abc import Set as SetABC
from dataclasses import dataclass
from pathlib import Path
from types import MappingProxyType
from typing import Any, Mapping

_PLACEHOLDER = re.compile(r"\{([^{}]+)\}")
_IMMUTABLE_PROVENANCE_SCALARS = (str, int, float, bool, type(None))


def _freeze_provenance(value: Any, active: set[int] | None = None) -> Any:
    """Detach and recursively freeze JSON-like provenance containers."""
    if isinstance(value, float) and not math.isfinite(value):
        raise ValueError("candidate provenance floats must be finite")
    if isinstance(value, _IMMUTABLE_PROVENANCE_SCALARS):
        return value

    active = set() if active is None else active
    identity = id(value)
    if identity in active:
        raise ValueError("candidate provenance must not contain cycles")

    active.add(identity)
    try:
        if isinstance(value, MappingABC):
            if any(not isinstance(key, str) for key in value):
                raise TypeError("candidate provenance mapping keys must be strings")
            return MappingProxyType(
                {
                    key: _freeze_provenance(item, active)
                    for key, item in value.items()
                }
            )
        if isinstance(value, (list, tuple)):
            return tuple(_freeze_provenance(item, active) for item in value)
        if isinstance(value, SetABC):
            return frozenset(
                _freeze_provenance(item, active) for item in value
            )
    finally:
        active.remove(identity)

    raise TypeError(
        "candidate provenance values must be JSON-compatible immutable "
        "scalars, mappings, sequences, or sets"
    )


@dataclass(frozen=True, slots=True)
class CommandCandidateSpec:
    """Immutable description of one argv-based OCR candidate."""

    id: str
    label: str
    argv: tuple[str, ...]
    environment: Mapping[str, str]
    provenance: Mapping[str, Any]

    def __post_init__(self) -> None:
        if not isinstance(self.id, str) or not self.id.strip():
            raise ValueError("candidate id must not be empty")
        if not isinstance(self.label, str) or not self.label.strip():
            raise ValueError("candidate label must not be empty")
        if not isinstance(self.argv, tuple) or not self.argv:
            raise ValueError("candidate argv must be a non-empty tuple")
        if any(not isinstance(argument, str) for argument in self.argv):
            raise TypeError("candidate argv values must be strings")

        placeholders = [
            match.group(1)
            for argument in self.argv
            for match in _PLACEHOLDER.finditer(argument)
        ]
        unknown = sorted(set(placeholders) - {"input"})
        if unknown:
            raise ValueError(
                f"unknown placeholder in candidate argv: {', '.join(unknown)}"
            )
        if self.argv.count("{input}") != 1 or placeholders.count("input") != 1:
            raise ValueError(
                "candidate argv must contain exactly one complete {input} argument"
            )

        try:
            environment = dict(self.environment)
        except (TypeError, ValueError) as error:
            raise TypeError("candidate environment must be a mapping") from error
        if any(
            not isinstance(key, str) or not isinstance(value, str)
            for key, value in environment.items()
        ):
            raise TypeError("candidate environment keys and values must be strings")
        if not isinstance(self.provenance, MappingABC):
            raise TypeError("candidate provenance must be a mapping")

        object.__setattr__(self, "environment", MappingProxyType(environment))
        object.__setattr__(
            self, "provenance", _freeze_provenance(self.provenance)
        )


def render_argv(
    spec: CommandCandidateSpec, input_path: str | Path
) -> list[str]:
    """Render the single input argument without shell parsing."""
    rendered = str(input_path)
    return [
        rendered if argument == "{input}" else argument
        for argument in spec.argv
    ]
