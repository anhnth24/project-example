"""Line-oriented worker for arbitrary argv-based benchmark candidates."""

from __future__ import annotations

import argparse
import json
import os
import selectors
import subprocess
import sys
import time
from pathlib import Path

import psutil

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
    process = subprocess.Popen(
        command,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if process.stdout is None or process.stderr is None:
        raise RuntimeError("candidate output pipes are unavailable")
    selector = selectors.DefaultSelector()
    streams = {
        process.stdout.fileno(): ("stdout", bytearray()),
        process.stderr.fileno(): ("stderr", bytearray()),
    }
    for descriptor in streams:
        os.set_blocking(descriptor, False)
        selector.register(descriptor, selectors.EVENT_READ)
    try:
        while selector.get_map():
            for key, _ in selector.select(timeout=0.05):
                descriptor = key.fd
                chunk = os.read(descriptor, 65536)
                if not chunk:
                    selector.unregister(descriptor)
                    continue
                streams[descriptor][1].extend(chunk)
                if len(streams[descriptor][1]) > max_output_bytes:
                    _terminate_candidate_tree(process)
                    raise OutputLimitExceeded
            if process.poll() is not None and not selector.get_map():
                break
        returncode = process.wait()
    finally:
        selector.close()
    if returncode != 0:
        raise RuntimeError("candidate command failed")
    stdout = streams[process.stdout.fileno()][1]
    return bytes(stdout).decode("utf-8", errors="replace")


def _terminate_candidate_tree(process: subprocess.Popen[bytes]) -> None:
    try:
        monitored = psutil.Process(process.pid)
        descendants = monitored.children(recursive=True)
    except (psutil.Error, ProcessLookupError):
        descendants = []
    for child in reversed(descendants):
        try:
            child.terminate()
        except (psutil.Error, ProcessLookupError):
            pass
    try:
        process.terminate()
    except ProcessLookupError:
        pass
    _, alive = psutil.wait_procs(descendants, timeout=0.5)
    for child in alive:
        try:
            child.kill()
        except (psutil.Error, ProcessLookupError):
            pass
    try:
        process.wait(timeout=0.5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()


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
