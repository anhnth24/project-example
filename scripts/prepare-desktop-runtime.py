#!/usr/bin/env python3
"""Prepare bundled PDFium + Tesseract runtime for a Tauri desktop build."""

from __future__ import annotations

import argparse
import json
import os
import platform
import re
import shutil
import stat
import subprocess
import tarfile
import tempfile
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DEST = ROOT / "app/src-tauri/native-runtime"
PDFIUM_BUILD = "7947"
PDFIUM_VERSION = "152.0.7947.0"
TESSDATA_COMMIT = "e12c65a915945e4c28e237a9b52bc4a8f39a0cec"
TESSERACT_WINDOWS_VERSION = "5.5.0.20241111"
MODELS = ("vie", "eng")


def run(*args: str, capture: bool = False) -> str:
    result = subprocess.run(
        args,
        check=True,
        text=True,
        capture_output=capture,
    )
    return result.stdout.strip() if capture else ""


def download(url: str, target: Path) -> None:
    request = urllib.request.Request(url, headers={"User-Agent": "markhand-build"})
    with urllib.request.urlopen(request, timeout=180) as response, target.open("wb") as out:
        shutil.copyfileobj(response, out)


def reset_destination() -> None:
    for child in DEST.iterdir():
        if child.name != "README.md":
            if child.is_dir():
                shutil.rmtree(child)
            else:
                child.unlink()
    (DEST / "ocr/bin").mkdir(parents=True)
    (DEST / "ocr/lib").mkdir(parents=True)
    (DEST / "ocr/tessdata").mkdir(parents=True)
    (DEST / "licenses").mkdir(parents=True)


def prepare_pdfium(system: str, architecture: str) -> None:
    if system == "linux":
        asset = "pdfium-linux-x64.tgz"
    elif system == "windows":
        asset = "pdfium-win-x64.tgz"
    elif system == "macos" and architecture in {"arm64", "aarch64"}:
        asset = "pdfium-mac-arm64.tgz"
    elif system == "macos":
        asset = "pdfium-mac-x64.tgz"
    else:
        raise RuntimeError(f"unsupported PDFium target: {system}/{architecture}")

    url = (
        "https://github.com/bblanchon/pdfium-binaries/releases/download/"
        f"chromium%2F{PDFIUM_BUILD}/{asset}"
    )
    with tempfile.TemporaryDirectory() as temporary:
        archive = Path(temporary) / asset
        download(url, archive)
        with tarfile.open(archive, "r:gz") as package:
            package.extractall(DEST / "pdfium", filter="data")
    version_text = (DEST / "pdfium/VERSION").read_text().strip()
    parts = dict(re.findall(r"([A-Z]+)=(\d+)", version_text))
    version = ".".join(parts[key] for key in ("MAJOR", "MINOR", "BUILD", "PATCH"))
    if version != PDFIUM_VERSION:
        raise RuntimeError(f"unexpected PDFium version: {version_text}")
    shutil.copy2(DEST / "pdfium/LICENSE", DEST / "licenses/PDFium-LICENSE")
    shutil.copytree(
        DEST / "pdfium/licenses",
        DEST / "licenses/pdfium-third-party",
        dirs_exist_ok=True,
    )


def prepare_models() -> None:
    for language in MODELS:
        download(
            "https://raw.githubusercontent.com/tesseract-ocr/tessdata_best/"
            f"{TESSDATA_COMMIT}/{language}.traineddata",
            DEST / f"ocr/tessdata/{language}.traineddata",
        )


def linux_library_dependencies(binary: Path) -> list[tuple[str, Path]]:
    output = run("ldd", str(binary), capture=True)
    dependencies = []
    for line in output.splitlines():
        match = re.match(r"\s*(\S+)\s+=>\s+(\S+)\s+\(", line)
        if match and Path(match.group(2)).is_file():
            dependencies.append((match.group(1), Path(match.group(2))))
    return dependencies


def prepare_linux_tesseract() -> str:
    executable = Path(shutil.which("tesseract") or "")
    if not executable.is_file():
        raise RuntimeError("tesseract is not installed")
    bundled = DEST / "ocr/bin/tesseract"
    shutil.copy2(executable, bundled)

    excluded = {
        "libc.so.6",
        "libdl.so.2",
        "libm.so.6",
        "libpthread.so.0",
        "librt.so.1",
        "ld-linux-x86-64.so.2",
    }
    pending = [bundled]
    visited: set[str] = set()
    while pending:
        current = pending.pop()
        for soname, source in linux_library_dependencies(current):
            if soname in excluded or soname in visited:
                continue
            visited.add(soname)
            target = DEST / "ocr/lib" / soname
            shutil.copy2(source.resolve(), target)
            pending.append(target)

    run("patchelf", "--set-rpath", "$ORIGIN/../lib", str(bundled))
    for library in (DEST / "ocr/lib").iterdir():
        run("patchelf", "--set-rpath", "$ORIGIN", str(library))

    copyright = Path("/usr/share/doc/tesseract-ocr/copyright")
    if copyright.is_file():
        shutil.copy2(copyright, DEST / "licenses/Tesseract-COPYRIGHT")
    return run(str(bundled), "--version", capture=True).splitlines()[0]


def prepare_windows_tesseract() -> str:
    source = Path(os.environ.get("ProgramFiles", r"C:\Program Files")) / "Tesseract-OCR"
    if not (source / "tesseract.exe").is_file():
        raise RuntimeError(f"Tesseract installation missing: {source}")
    for item in source.iterdir():
        if item.name.lower() == "tessdata":
            continue
        if item.is_file() and (item.suffix.lower() == ".dll" or item.name == "tesseract.exe"):
            shutil.copy2(item, DEST / "ocr/bin" / item.name)
    installed_data = source / "tessdata"
    for name in ("configs", "tessconfigs", "pdf.ttf"):
        item = installed_data / name
        target = DEST / "ocr/tessdata" / name
        if item.is_dir():
            shutil.copytree(item, target, dirs_exist_ok=True)
        elif item.is_file():
            shutil.copy2(item, target)
    for license_name in ("LICENSE", "README.md"):
        license_path = source / license_name
        if license_path.is_file():
            shutil.copy2(
                license_path, DEST / "licenses" / f"Tesseract-{license_name}"
            )
    executable = DEST / "ocr/bin/tesseract.exe"
    return run(str(executable), "--version", capture=True).splitlines()[0]


def prepare_macos_tesseract() -> str:
    prefix = Path(run("brew", "--prefix", "tesseract", capture=True))
    source = prefix / "bin/tesseract"
    bundled = DEST / "ocr/bin/tesseract"
    shutil.copy2(source, bundled)
    run(
        "dylibbundler",
        "-od",
        "-b",
        "-x",
        str(bundled),
        "-d",
        str(DEST / "ocr/lib"),
        "-p",
        "@executable_path/../lib",
    )
    for license_name in ("LICENSE", "COPYING"):
        license_path = prefix / license_name
        if license_path.is_file():
            shutil.copy2(
                license_path, DEST / "licenses" / f"Tesseract-{license_name}"
            )
    return run(str(bundled), "--version", capture=True).splitlines()[0]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--platform",
        choices=("linux", "windows", "macos"),
        default={"Darwin": "macos", "Windows": "windows"}.get(
            platform.system(), "linux"
        ),
    )
    args = parser.parse_args()

    reset_destination()
    architecture = platform.machine().lower()
    prepare_pdfium(args.platform, architecture)
    prepare_models()
    if args.platform == "linux":
        tesseract_version = prepare_linux_tesseract()
    elif args.platform == "windows":
        tesseract_version = prepare_windows_tesseract()
    else:
        tesseract_version = prepare_macos_tesseract()

    for executable in (DEST / "ocr/bin").glob("*"):
        executable.chmod(executable.stat().st_mode | stat.S_IXUSR)
    manifest = {
        "platform": args.platform,
        "architecture": architecture,
        "pdfium": PDFIUM_VERSION,
        "tesseract": tesseract_version,
        "tessdata_commit": TESSDATA_COMMIT,
        "languages": list(MODELS),
    }
    (DEST / "runtime-manifest.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n"
    )
    print(json.dumps(manifest, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
