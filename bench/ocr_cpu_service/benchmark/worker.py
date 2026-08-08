"""Line-oriented worker for arbitrary argv-based benchmark candidates."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import time
from pathlib import Path

def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser()
    parser.add_argument("--argv-json", required=True)
    return parser


def _parse_candidate_argv(encoded: str) -> tuple[str, ...]:
    values = json.loads(encoded)
    if (
        not isinstance(values, list)
        or not values
        or any(not isinstance(value, str) for value in values)
        or values.count("{input}") != 1
    ):
        raise ValueError("candidate argv must contain one complete {input} argument")
    return tuple(values)


def _recognize(argv: tuple[str, ...], path: Path) -> str:
    command = [str(path) if value == "{input}" else value for value in argv]
    result = subprocess.run(
        command,
        capture_output=True,
        text=True,
        check=True,
    )
    return result.stdout


def main() -> None:
    args = _parser().parse_args()
    started = time.perf_counter()
    argv = _parse_candidate_argv(args.argv_json)
    print(
        json.dumps(
            {
                "event": "ready",
                "candidate_seconds": time.perf_counter() - started,
            }
        ),
        flush=True,
    )

    for line in sys.stdin:
        request = json.loads(line)
        if request.get("event") == "shutdown":
            return
        if request.get("event") != "recognize":
            raise ValueError("unsupported worker event")
        started = time.perf_counter()
        text = _recognize(argv, Path(request["path"]))
        print(
            json.dumps(
                {
                    "event": "result",
                    "text": text,
                    "candidate_seconds": time.perf_counter() - started,
                },
                ensure_ascii=False,
            ),
            flush=True,
        )


if __name__ == "__main__":
    main()
