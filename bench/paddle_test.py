#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Test chất lượng PaddleOCR tiếng Việt trên ảnh, in text theo thứ tự đọc.
Dùng: python paddle_test.py <image> [lang]   (lang mặc định 'vi')
Sắp xếp box theo hàng (top→bottom) rồi trong hàng (left→right).
"""
import sys

def run(img, lang="vi"):
    from paddleocr import PaddleOCR
    try:
        ocr = PaddleOCR(use_angle_cls=True, lang=lang, show_log=False)
    except TypeError:
        ocr = PaddleOCR(lang=lang)  # API mới có thể bỏ tham số

    # API cũ: .ocr() trả [[ [box,(text,conf)], ... ]]
    items = []
    try:
        res = ocr.ocr(img, cls=True)
        page = res[0] if res and isinstance(res, list) else res
        for line in page:
            box, (text, conf) = line[0], line[1]
            ys = [p[1] for p in box]; xs = [p[0] for p in box]
            items.append((min(ys), min(xs), text))
    except Exception:
        # API mới (3.x): predict() trả dict có 'rec_texts','rec_polys'
        res = ocr.predict(img)
        r = res[0]
        texts = r["rec_texts"]; polys = r["rec_polys"]
        for poly, text in zip(polys, texts):
            ys = [p[1] for p in poly]; xs = [p[0] for p in poly]
            items.append((min(ys), min(xs), text))

    # Gom thành hàng: sắp theo y, ngắt hàng khi y nhảy > ngưỡng
    items.sort(key=lambda t: (t[0], t[1]))
    lines, cur, last_y = [], [], None
    for y, x, text in items:
        if last_y is None or abs(y - last_y) <= 15:
            cur.append((x, text)); last_y = y if last_y is None else last_y
        else:
            lines.append(cur); cur = [(x, text)]; last_y = y
    if cur: lines.append(cur)
    out = []
    for ln in lines:
        ln.sort(key=lambda t: t[0])
        out.append(" ".join(t for _, t in ln))
    return "\n".join(out)

if __name__ == "__main__":
    img = sys.argv[1]
    lang = sys.argv[2] if len(sys.argv) > 2 else "vi"
    print(run(img, lang))
