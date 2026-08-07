"""Model-neutral command candidate specifications."""

from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path
from types import MappingProxyType
from typing import Any, Mapping

_PLACEHOLDER = re.compile(r"\{([^{}]+)\}")


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
        try:
            provenance = dict(self.provenance)
        except (TypeError, ValueError) as error:
            raise TypeError("candidate provenance must be a mapping") from error

        object.__setattr__(self, "environment", MappingProxyType(environment))
        object.__setattr__(self, "provenance", MappingProxyType(provenance))


def render_argv(
    spec: CommandCandidateSpec, input_path: str | Path
) -> list[str]:
    """Render the single input argument without shell parsing."""
    rendered = str(input_path)
    return [
        rendered if argument == "{input}" else argument
        for argument in spec.argv
    ]
