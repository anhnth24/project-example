#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Sweep cấu hình OCR Tesseract để tìm cách tăng độ chính xác tiếng Việt.
So CER với ground-truth (ref.txt). Không sửa backend — chỉ đo bằng tesseract CLI.
"""
import subprocess, tempfile, os, sys

ROOT = os.path.join(os.path.dirname(__file__), "vn_corpus")
REF = open(os.path.join(ROOT, "ref.txt"), encoding="utf-8").read()
BEST = os.path.join(os.path.dirname(__file__), "..", "tessdata_best")

def norm(s):
    return " ".join(s.split())

def lev(a, b):
    a, b = list(a), list(b)
    if not a: return len(b)
    if not b: return len(a)
    prev = list(range(len(b) + 1))
    for i in range(1, len(a) + 1):
        cur = [i] + [0] * len(b)
        for j in range(1, len(b) + 1):
            cur[j] = min(prev[j] + 1, cur[j-1] + 1, prev[j-1] + (a[i-1] != b[j-1]))
        prev = cur
    return prev[-1]

def cer(ref, hyp):
    r, h = norm(ref), norm(hyp)
    return lev(r, h) / max(1, len(r))

def preprocess(img, ops):
    if not ops:
        return img
    out = tempfile.mktemp(suffix=".png")
    subprocess.run(["convert", img] + ops + [out], check=True)
    return out

def ocr(img, lang="vie+eng", psm="3", best=False, ops=None):
    p = preprocess(img, ops)
    env = dict(os.environ)
    if best:
        env["TESSDATA_PREFIX"] = os.path.abspath(BEST)
    r = subprocess.run(["tesseract", p, "stdout", "-l", lang, "--psm", psm],
                        capture_output=True, env=env)
    if ops:
        try: os.remove(p)
        except OSError: pass
    return r.stdout.decode("utf-8", "ignore")

IMAGES = {
    "printed": os.path.join(ROOT, "vn_printed.png"),
    "lowres":  os.path.join(ROOT, "vn_lowres.png"),
}

UP = ["-colorspace","Gray","-resize","200%","-unsharp","0x1","-normalize"]
CONFIGS = [
    ("baseline (fast, vie+eng, psm3)",        dict(lang="vie+eng", psm="3", best=False)),
    ("fast + psm6",                            dict(lang="vie+eng", psm="6", best=False)),
    ("fast + upscale2x",                       dict(lang="vie+eng", psm="3", best=False, ops=UP)),
    ("fast + psm6 + upscale2x",                dict(lang="vie+eng", psm="6", best=False, ops=UP)),
    ("best + psm6",                            dict(lang="vie+eng", psm="6", best=True)),
    ("best + psm6 + upscale2x",                dict(lang="vie+eng", psm="6", best=True, ops=UP)),
    ("best + vie only + psm6",                 dict(lang="vie",     psm="6", best=True)),
]

print(f"{'config':42s} | " + " | ".join(f"{k} CER%" for k in IMAGES))
print("-"*80)
for name, cfg in CONFIGS:
    row = []
    for _, img in IMAGES.items():
        try:
            c = cer(REF, ocr(img, **cfg)) * 100
            row.append(f"{100-c:6.1f}")  # accuracy %
        except Exception as e:
            row.append(f" ERR")
    print(f"{name:42s} | " + " | ".join(f"{v}%" for v in row))
