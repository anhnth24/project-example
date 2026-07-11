#!/usr/bin/env python3
"""Small JSON bridge used by fileconv-core's optional PaddleOCR tier."""

from __future__ import annotations

import argparse
import json
import sys
from typing import Any


def reading_order(polys: list[Any], texts: list[str]) -> list[dict[str, Any]]:
    items: list[dict[str, Any]] = []
    for poly, text in zip(polys, texts):
        xs = [float(point[0]) for point in poly]
        ys = [float(point[1]) for point in poly]
        items.append(
            {
                "text": str(text),
                "x0": min(xs),
                "x1": max(xs),
                "y0": min(ys),
                "y1": max(ys),
                "cx": (min(xs) + max(xs)) / 2,
            }
        )
    if len(items) < 4:
        return sorted(items, key=lambda item: (item["y0"], item["x0"]))

    ordered_x = sorted(items, key=lambda item: item["cx"])
    page_width = max(item["x1"] for item in items) - min(item["x0"] for item in items)
    gaps = [
        (ordered_x[index + 1]["cx"] - ordered_x[index]["cx"], index)
        for index in range(len(ordered_x) - 1)
    ]
    gap, split = max(gaps, default=(0.0, 0))
    left = ordered_x[: split + 1]
    right = ordered_x[split + 1 :]
    if gap > page_width * 0.2 and len(left) >= 2 and len(right) >= 2:
        return sorted(left, key=lambda item: (item["y0"], item["x0"])) + sorted(
            right, key=lambda item: (item["y0"], item["x0"])
        )
    return sorted(items, key=lambda item: (item["y0"], item["x0"]))


def run(image: str, lang: str) -> dict[str, Any]:
    from paddleocr import PaddleOCR

    ocr = PaddleOCR(
        lang=lang,
        enable_mkldnn=False,
        use_doc_orientation_classify=False,
        use_doc_unwarping=False,
        use_textline_orientation=False,
    )
    pages = ocr.predict(image)
    items: list[dict[str, Any]] = []
    for page in pages:
        texts = list(page["rec_texts"])
        polys = page["rec_polys"] if "rec_polys" in page else page["dt_polys"]
        items.extend(reading_order(list(polys), texts))
    return {
        "engine": "paddle",
        "text": "\n".join(item["text"].strip() for item in items if item["text"].strip()),
        "lines": items,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--image", required=True)
    parser.add_argument("--lang", default="vi")
    args = parser.parse_args()
    try:
        print(json.dumps(run(args.image, args.lang), ensure_ascii=False))
    except Exception as error:  # Rust receives a stable non-zero fallback signal.
        print(json.dumps({"error": str(error)}, ensure_ascii=False), file=sys.stderr)
        raise SystemExit(2)


if __name__ == "__main__":
    main()
