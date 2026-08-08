"""Line-oriented worker for arbitrary argv-based benchmark candidates."""

from __future__ import annotations

import argparse
import json
import resource
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

DEFAULT_MAX_OUTPUT_BYTES = 1024 * 1024


class OutputLimitExceeded(RuntimeError):
    """Candidate stdout or stderr reached its hard byte limit."""


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser()
    parser.add_argument("--argv-json", required=True)
    parser.add_argument(
        "--max-output-bytes",
        type=int,
        default=DEFAULT_MAX_OUTPUT_BYTES,
    )
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


def _recognize(
    argv: tuple[str, ...], path: Path, *, max_output_bytes: int
) -> str:
    if max_output_bytes <= 0:
        raise ValueError("max_output_bytes must be positive")
    command = [str(path) if value == "{input}" else value for value in argv]
    with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:

        def limit_output_files() -> None:
            resource.setrlimit(
                resource.RLIMIT_FSIZE,
                (max_output_bytes, max_output_bytes),
            )

        result = subprocess.run(
            command,
            stdout=stdout,
            stderr=stderr,
            check=False,
            preexec_fn=limit_output_files,
        )
        stdout_bytes = stdout.tell()
        stderr_bytes = stderr.tell()
        if (
            stdout_bytes >= max_output_bytes
            or stderr_bytes >= max_output_bytes
            or result.returncode == -signal.SIGXFSZ
        ):
            raise OutputLimitExceeded
        if result.returncode != 0:
            raise RuntimeError("candidate command failed")
        stdout.seek(0)
        return stdout.read(max_output_bytes).decode("utf-8", errors="replace")


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
        try:
            text = _recognize(
                argv,
                Path(request["path"]),
                max_output_bytes=args.max_output_bytes,
            )
        except OutputLimitExceeded:
            print(
                json.dumps(
                    {
                        "event": "failure",
                        "error_kind": "output_limit",
                        "candidate_seconds": time.perf_counter() - started,
                    }
                ),
                flush=True,
            )
            continue
        except Exception:
            print(
                json.dumps(
                    {
                        "event": "failure",
                        "error_kind": "candidate_error",
                        "candidate_seconds": time.perf_counter() - started,
                    }
                ),
                flush=True,
            )
            continue
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
