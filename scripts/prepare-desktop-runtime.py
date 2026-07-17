#!/usr/bin/env python3
"""Prepare bundled PDFium + Tesseract runtime for a Tauri desktop build."""

from __future__ import annotations

import argparse
import hashlib
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
DOWNLOAD_SHA256 = {
    "pdfium-linux-x64.tgz": "f73d69d309fe1f33cc7269dcc99be31ec44e1cf608e31d7e2fcc6545fc2f9323",
    "pdfium-win-x64.tgz": "75df6802fc090ad7c76ccc29ed80c3fcb1a375c775bbf8e522189174647b101f",
    "pdfium-mac-arm64.tgz": "aa9739354fc7bc8f200f3f3c9532bd5233298203051e094820272ccd9c997a77",
    "pdfium-mac-x64.tgz": "16d7a263b9e2f550d230ce81637697381b0ce898f2e3a22c7316594b15199d87",
    "eng.traineddata": "8280aed0782fe27257a68ea10fe7ef324ca0f8d85bd2fd145d1c2b560bcb66ba",
    "vie.traineddata": "b6b49293d95d0b6dbd8780174627e82c75be957b6f4ed9862155540d6b00bb45",
    "tessdata-LICENSE": "a6cba85bc92e0cff7a450b1d873c0eaa2e9fc96bf472df0247a26bec77bf3ff9",
    "tesseract-LICENSE": "cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30",
}


def run(*args: str, capture: bool = False) -> str:
    result = subprocess.run(
        args,
        check=True,
        text=True,
        capture_output=capture,
    )
    return result.stdout.strip() if capture else ""


def validate_tesseract_version(version: str, default_pattern: str) -> None:
    expected = os.environ.get("FILECONV_TESSERACT_VERSION")
    pattern = re.escape(expected) if expected else default_pattern
    if not re.search(pattern, version):
        requirement = expected or default_pattern
        raise RuntimeError(f"expected Tesseract {requirement}, got {version}")


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


def prepare_models() -> None:
    for language in MODELS:
        filename = f"{language}.traineddata"
        download(
            "https://raw.githubusercontent.com/tesseract-ocr/tessdata_best/"
            f"{TESSDATA_COMMIT}/{filename}",
            DEST / f"ocr/tessdata/{filename}",
            filename,
        )
    download(
        "https://raw.githubusercontent.com/tesseract-ocr/tessdata_best/"
        f"{TESSDATA_COMMIT}/LICENSE",
        DEST / "licenses/tessdata-LICENSE",
        "tessdata-LICENSE",
    )
    download(
        "https://raw.githubusercontent.com/tesseract-ocr/tesseract/5.5.2/LICENSE",
        DEST / "licenses/Tesseract-LICENSE",
        "tesseract-LICENSE",
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
    packages: set[str] = {"tesseract-ocr"}
    while pending:
        current = pending.pop()
        for soname, source in linux_library_dependencies(current):
            if soname in excluded or soname in visited:
                continue
            visited.add(soname)
            target = DEST / "ocr/lib" / soname
            shutil.copy2(source.resolve(), target)
            pending.append(target)
            owner = subprocess.run(
                ("dpkg-query", "-S", str(source.resolve())),
                capture_output=True,
                text=True,
                check=False,
            )
            if owner.returncode == 0:
                packages.add(owner.stdout.split(":", 1)[0])

    run("patchelf", "--set-rpath", "$ORIGIN/../lib", str(bundled))
    for library in (DEST / "ocr/lib").iterdir():
        run("patchelf", "--set-rpath", "$ORIGIN", str(library))

    copyright = Path("/usr/share/doc/tesseract-ocr/copyright")
    if copyright.is_file():
        shutil.copy2(copyright, DEST / "licenses/Tesseract-COPYRIGHT")
    for package in sorted(packages):
        notice = Path("/usr/share/doc") / package / "copyright"
        if notice.is_file():
            safe_name = package.replace(":", "_")
            shutil.copy2(notice, DEST / "licenses" / f"linux-{safe_name}-copyright")
    (DEST / "licenses/linux-packages.json").write_text(
        json.dumps(sorted(packages), indent=2) + "\n"
    )
    version = run(str(bundled), "--version", capture=True).splitlines()[0]
    validate_tesseract_version(version, r"\b[45]\.")
    return version


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
    version = run(str(executable), "--version", capture=True).splitlines()[0]
    windows_semver = ".".join(TESSERACT_WINDOWS_VERSION.split(".")[:3])
    validate_tesseract_version(version, rf"\b{re.escape(windows_semver)}\b")
    return version


def prepare_macos_tesseract() -> str:
    prefix = Path(run("brew", "--prefix", "tesseract", capture=True))
    source = prefix / "bin/tesseract"
    version = run(str(source), "--version", capture=True).splitlines()[0]
    validate_tesseract_version(version, r"\b5\.")
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
        "@executable_path/../Frameworks",
    )
    for license_name in ("LICENSE", "COPYING"):
        license_path = prefix / license_name
        if license_path.is_file():
            shutil.copy2(
                license_path, DEST / "licenses" / f"Tesseract-{license_name}"
            )
    dependencies = run("brew", "deps", "--include-build", "tesseract", capture=True).split()
    formulae = sorted(set(["tesseract", *dependencies]))
    brew_inventory = run("brew", "info", "--json=v2", *formulae, capture=True)
    (DEST / "licenses/macos-homebrew-formulae.json").write_text(
        json.dumps(json.loads(brew_inventory), indent=2) + "\n"
    )
    architecture = platform.machine().lower()
    triple = (
        "aarch64-apple-darwin"
        if architecture in {"arm64", "aarch64"}
        else "x86_64-apple-darwin"
    )
    sidecar = DEST / "sidecars" / f"tesseract-{triple}"
    sidecar.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(bundled, sidecar)

    config_path = ROOT / "app/src-tauri/tauri.macos.conf.json"
    config = json.loads(config_path.read_text())
    bundle = config.setdefault("bundle", {})
    bundle["externalBin"] = ["native-runtime/sidecars/tesseract"]
    bundle["resources"] = {
        "native-runtime/ocr/tessdata/": "native-runtime/ocr/tessdata/",
        "native-runtime/licenses/": "native-runtime/licenses/",
        "native-runtime/runtime-manifest.json": "native-runtime/runtime-manifest.json",
    }
    macos = bundle.setdefault("macOS", {})
    frameworks = [DEST / "pdfium/lib/libpdfium.dylib"]
    frameworks.extend(
        path for path in (DEST / "ocr/lib").iterdir() if path.suffix == ".dylib"
    )
    macos["frameworks"] = [
        path.relative_to(ROOT / "app/src-tauri").as_posix()
        for path in sorted(frameworks)
    ]
    config_path.write_text(json.dumps(config, indent=2) + "\n")
    return version


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
