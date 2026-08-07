from __future__ import annotations

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parents[1]))

from benchmark.candidates import CommandCandidateSpec, render_argv  # noqa: E402


def test_renders_argv_without_a_shell(tmp_path: Path) -> None:
    spec = CommandCandidateSpec(
        id="baseline",
        label="Baseline",
        argv=("fileconv", "one", "{input}"),
        environment={"FILECONV_TESSDATA": "/models"},
        provenance={},
    )

    assert render_argv(spec, tmp_path / "a b.png") == [
        "fileconv",
        "one",
        str(tmp_path / "a b.png"),
    ]


@pytest.mark.parametrize(
    "argv",
    [
        ("tool",),
        ("tool", "{input}", "{input}"),
        ("tool", "prefix-{input}"),
    ],
)
def test_rejects_missing_multiple_or_partial_input_placeholders(
    argv: tuple[str, ...],
) -> None:
    with pytest.raises(ValueError, match="exactly one"):
        CommandCandidateSpec("x", "X", argv, {}, {})


def test_rejects_unknown_placeholders() -> None:
    with pytest.raises(ValueError, match="unknown placeholder"):
        CommandCandidateSpec("x", "X", ("tool", "{input}", "{output}"), {}, {})


@pytest.mark.parametrize(
    ("values", "message"),
    [
        ({"id": "", "label": "X", "argv": ("tool", "{input}")}, "id"),
        ({"id": "x", "label": " ", "argv": ("tool", "{input}")}, "label"),
        ({"id": "x", "label": "X", "argv": ()}, "argv"),
        ({"id": "x", "label": "X", "argv": ("tool", 1, "{input}")}, "argv"),
    ],
)
def test_rejects_empty_identity_or_invalid_argv(
    values: dict[str, object], message: str
) -> None:
    with pytest.raises((TypeError, ValueError), match=message):
        CommandCandidateSpec(
            values["id"],  # type: ignore[arg-type]
            values["label"],  # type: ignore[arg-type]
            values["argv"],  # type: ignore[arg-type]
            {},
            {},
        )


@pytest.mark.parametrize(
    "environment",
    [
        {"THREADS": 4},
        {1: "value"},
    ],
)
def test_rejects_non_string_environment_entries(
    environment: dict[object, object],
) -> None:
    with pytest.raises((TypeError, ValueError), match="environment"):
        CommandCandidateSpec(
            "x",
            "X",
            ("tool", "{input}"),
            environment,  # type: ignore[arg-type]
            {},
        )


def test_candidate_spec_is_immutable() -> None:
    spec = CommandCandidateSpec(
        "x",
        "X",
        ("tool", "{input}"),
        {"THREADS": "4"},
        {"version": "1"},
    )

    with pytest.raises((AttributeError, TypeError)):
        spec.environment["THREADS"] = "8"  # type: ignore[index]
    with pytest.raises((AttributeError, TypeError)):
        spec.provenance["version"] = "2"  # type: ignore[index]


def test_provenance_nested_structures_are_immutable() -> None:
    spec = CommandCandidateSpec(
        "x",
        "X",
        ("tool", "{input}"),
        {},
        {
            "build": {
                "assets": [{"name": "detector"}],
                "features": {"cpu", "offline"},
            }
        },
    )
    build = spec.provenance["build"]

    with pytest.raises(TypeError):
        build["profile"] = "debug"  # type: ignore[index]
    with pytest.raises(AttributeError):
        build["assets"].append({"name": "recognizer"})  # type: ignore[union-attr]
    with pytest.raises(AttributeError):
        build["features"].add("network")  # type: ignore[union-attr]


def test_provenance_is_detached_from_caller_nested_structures() -> None:
    assets = [{"name": "detector"}]
    features = {"cpu", "offline"}
    build = {"assets": assets, "features": features}
    provenance = {"build": build}

    spec = CommandCandidateSpec(
        "x",
        "X",
        ("tool", "{input}"),
        {},
        provenance,
    )
    assets[0]["name"] = "mutated"
    assets.append({"name": "recognizer"})
    features.add("network")
    build["profile"] = "debug"

    frozen_build = spec.provenance["build"]
    assert frozen_build["assets"] == ({"name": "detector"},)
    assert frozen_build["features"] == frozenset({"cpu", "offline"})
    assert "profile" not in frozen_build
