"""Line-oriented isolated candidate worker for comparable measurements."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import time
from pathlib import Path
from typing import Protocol

from PIL import Image

from markhand_ocr.ordering import order_spans


class Recognizer(Protocol):
    def __call__(self, path: Path) -> str:
        """Recognize one already-rendered benchmark page."""


def _markhand_recognizer(
    fileconv: Path, tessdata: Path
) -> Recognizer:
    def recognize(path: Path) -> str:
        result = subprocess.run(
            [str(fileconv), "one", str(path), "--lang", "vie+eng"],
            capture_output=True,
            text=True,
            env={
                **dict(__import__("os").environ),
                "FILECONV_TESSDATA": str(tessdata),
            },
            check=True,
        )
        return result.stdout

    return recognize


def _paddle_recognizer(
    detection_dir: Path, recognition_dir: Path
) -> Recognizer:
    from markhand_ocr.paddle_backend import PaddleOcrBackend

    backend = PaddleOcrBackend(
        text_detection_model_dir=detection_dir,
        text_recognition_model_dir=recognition_dir,
    )

    def recognize(path: Path) -> str:
        with Image.open(path) as source:
            image = source.convert("RGB")
            try:
                spans = backend.recognize(image)
                ordered = order_spans(spans, page_width=image.width)
            finally:
                image.close()
        return "\n".join(span.text for span in ordered)

    return recognize


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--candidate",
        required=True,
        choices=("markhand-default", "markhand-tessdata-best", "pp-ocrv6"),
    )
    parser.add_argument("--fileconv", type=Path)
    parser.add_argument("--tessdata", type=Path)
    parser.add_argument("--detection-dir", type=Path)
    parser.add_argument("--recognition-dir", type=Path)
    return parser


def main() -> None:
    args = _parser().parse_args()
    started = time.perf_counter()
    if args.candidate.startswith("markhand-"):
        if args.fileconv is None or args.tessdata is None:
            raise ValueError("Markhand worker requires fileconv and tessdata")
        recognize = _markhand_recognizer(args.fileconv, args.tessdata)
    else:
        if args.detection_dir is None or args.recognition_dir is None:
            raise ValueError("Paddle worker requires both model directories")
        recognize = _paddle_recognizer(
            args.detection_dir, args.recognition_dir
        )
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
        text = recognize(Path(request["path"]))
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
