#!/usr/bin/env python3
"""Prepare the bundled PDFium runtime for a Tauri desktop build.

Tesseract/tessdata are no longer bundled: image/scan OCR runs through a
vision LLM (`FILECONV_OCR_*`, ADR 0016), so the only native document runtime
shipped with the desktop app is PDFium.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import platform
import re
import shutil
import tarfile
import tempfile
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DEST = ROOT / "app/src-tauri/native-runtime"
PDFIUM_BUILD = "7947"
PDFIUM_VERSION = "152.0.7947.0"
DOWNLOAD_SHA256 = {
    "pdfium-linux-x64.tgz": "f73d69d309fe1f33cc7269dcc99be31ec44e1cf608e31d7e2fcc6545fc2f9323",
    "pdfium-win-x64.tgz": "75df6802fc090ad7c76ccc29ed80c3fcb1a375c775bbf8e522189174647b101f",
    "pdfium-mac-arm64.tgz": "aa9739354fc7bc8f200f3f3c9532bd5233298203051e094820272ccd9c997a77",
    "pdfium-mac-x64.tgz": "16d7a263b9e2f550d230ce81637697381b0ce898f2e3a22c7316594b15199d87",
}


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def download(url: str, target: Path, lock_name: str) -> None:
    request = urllib.request.Request(url, headers={"User-Agent": "markhand-build"})
    with urllib.request.urlopen(request, timeout=180) as response, target.open("wb") as out:
        shutil.copyfileobj(response, out)
    actual = file_sha256(target)
    expected = DOWNLOAD_SHA256[lock_name]
    if actual != expected:
        target.unlink(missing_ok=True)
        raise RuntimeError(
            f"checksum mismatch for {lock_name}: expected {expected}, got {actual}"
        )


def reset_destination() -> None:
    for child in DEST.iterdir():
        if child.name != "README.md":
            if child.is_dir():
                shutil.rmtree(child)
            else:
                child.unlink()
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
        download(url, archive, asset)
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


def configure_macos_bundle() -> None:
    """Pin the PDFium dylib as a bundled framework in tauri.macos.conf.json.

    macOS cannot bundle the whole `native-runtime/` directory the way Linux
    and Windows do — the dylib must be listed under `bundle.macOS.frameworks`
    so it lands in `Contents/Frameworks/` (where `lib.rs` resolves it).
    """
    config_path = ROOT / "app/src-tauri/tauri.macos.conf.json"
    config = json.loads(config_path.read_text())
    bundle = config.setdefault("bundle", {})
    bundle["resources"] = {
        "native-runtime/licenses/": "native-runtime/licenses/",
        "native-runtime/runtime-manifest.json": "native-runtime/runtime-manifest.json",
    }
    macos = bundle.setdefault("macOS", {})
    macos["frameworks"] = [
        (DEST / "pdfium/lib/libpdfium.dylib")
        .relative_to(ROOT / "app/src-tauri")
        .as_posix()
    ]
    config_path.write_text(json.dumps(config, indent=2) + "\n")


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
    if args.platform == "macos":
        configure_macos_bundle()

    manifest = {
        "platform": args.platform,
        "architecture": architecture,
        "pdfium": PDFIUM_VERSION,
    }
    manifest["files"] = {
        path.relative_to(DEST).as_posix(): file_sha256(path)
        for path in sorted(DEST.rglob("*"))
        if path.is_file() and path.name not in {"README.md", "runtime-manifest.json"}
    }
    (DEST / "runtime-manifest.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n"
    )
    print(json.dumps(manifest, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
