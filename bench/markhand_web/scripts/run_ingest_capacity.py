#!/usr/bin/env python3
"""P0-08 converter ingest capacity harness.

This harness measures the checked-out runner only. It is suitable for local-cpu
smoke/sizing evidence and deliberately does not claim Profile B capacity.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import datetime as dt
import hashlib
import json
import os
import platform
import re
import shutil
import signal
import statistics
import subprocess
import sys
import time
import zipfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
CORPUS = ROOT / "bench/markhand_web"
MANIFEST_PATH = CORPUS / "golden/manifest.json"
WORKLOAD_PROFILE = CORPUS / "workload-profile.yaml"
SUMMARY_PATH = CORPUS / "ingest/summary.json"
REPORT_PATH = CORPUS / "reports/ingest-capacity.md"
DEFAULT_FILECONV = ROOT / "target/release/fileconv"
TARGET_DOCS_PER_HOUR = 1200.0
HEADROOM_TARGET_PERCENT = 30.0
CONCURRENT_WORKERS = 2
CONVERSION_TIMEOUT_SECONDS = 180.0
DOES_NOT_CLAIM = "does NOT claim Profile B G0-CAP-INGEST-THROUGHPUT pass evidence"


IMPLEMENTATION_FILES = (
    "bench/markhand_web/ingest/README.md",
    "bench/markhand_web/scripts/run_ingest_capacity.py",
    "bench/markhand_web/scripts/run_ingest_capacity.sh",
)


class HarnessError(RuntimeError):
    """Clear, actionable error for missing local prerequisites."""


def load_json(path: Path) -> dict:
    payload = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(payload, dict):
        raise HarnessError(f"invalid object payload: {path}")
    return payload


def git(*args: str) -> str:
    try:
        return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def git_status() -> dict:
    commit = git("rev-parse", "HEAD")
    branch = git("branch", "--show-current")
    raw = ""
    try:
        raw = subprocess.check_output(
            ["git", "status", "--porcelain"],
            cwd=ROOT,
            text=True,
        )
    except (OSError, subprocess.CalledProcessError):
        pass
    dirty_paths: list[str] = []
    for line in raw.splitlines():
        if len(line) < 4:
            continue
        path = line[3:]
        if " -> " in path:
            path = path.split(" -> ", 1)[1]
        if path.startswith('"') and path.endswith('"'):
            path = path[1:-1]
        dirty_paths.append(path)
    return {
        "commit": commit,
        "branch": branch,
        "dirty": bool(dirty_paths),
        "dirtyPaths": dirty_paths,
    }


def parse_key_value_file(path: Path) -> dict[str, str]:
    values: dict[str, str] = {}
    if not path.is_file():
        return values
    for line in path.read_text(errors="replace").splitlines():
        if "=" not in line:
            continue
        key, value = line.split("=", 1)
        values[key] = value.strip().strip('"')
    return values


def default_network_interface() -> str:
    route_path = Path("/proc/net/route")
    if not route_path.is_file():
        return "unknown"
    for line in route_path.read_text(errors="replace").splitlines()[1:]:
        parts = line.split()
        if len(parts) > 2 and parts[1] == "00000000":
            return parts[0]
    return "unknown"


def hardware_fingerprint(storage_path: Path) -> dict:
    cpuinfo = Path("/proc/cpuinfo").read_text(errors="replace") if Path("/proc/cpuinfo").is_file() else ""
    meminfo = Path("/proc/meminfo").read_text(errors="replace") if Path("/proc/meminfo").is_file() else ""
    vendor = re.search(r"vendor_id\s*:\s*(.+)", cpuinfo)
    model = re.search(r"model name\s*:\s*(.+)", cpuinfo)
    processors = re.findall(r"^processor\s*:", cpuinfo, re.MULTILINE)
    physical_cores = {
        (physical, core)
        for physical, core in re.findall(
            r"physical id\s*:\s*(\d+).*?core id\s*:\s*(\d+)",
            cpuinfo,
            re.DOTALL,
        )
    }
    memory = re.search(r"MemTotal:\s*(\d+)", meminfo)
    disk = shutil.disk_usage(storage_path)
    iface = default_network_interface()
    speed_path = Path("/sys/class/net") / iface / "speed"
    bandwidth = 0.0
    if speed_path.is_file():
        try:
            speed = int(speed_path.read_text().strip())
            bandwidth = speed / 1000 if speed > 0 else 0.0
        except (OSError, ValueError):
            pass
    os_release = parse_key_value_file(Path("/etc/os-release"))
    return {
        "cpu": {
            "vendor": vendor.group(1).strip() if vendor else "unknown",
            "model": model.group(1).strip() if model else "unknown",
            "cores": len(physical_cores) or len(processors) or (os.cpu_count() or 1),
            "threads": len(processors) or (os.cpu_count() or 1),
            "physicalCoresMeasured": bool(physical_cores),
        },
        "ramGb": round(int(memory.group(1)) * 1024 / 1_000_000_000, 2) if memory else 0.0,
        "disk": {
            "capacityGb": round(disk.total / 1_000_000_000, 2),
            "storagePathSha256": hashlib.sha256(str(storage_path.resolve()).encode()).hexdigest(),
            "type": "local",
        },
        "gpu": {"model": "unknown", "vramGb": None, "count": None},
        "network": {
            "interface": iface,
            "bandwidthGbps": bandwidth,
            "bandwidthMeasured": bandwidth > 0,
        },
        "os": {
            "distro": (
                f"{os_release.get('ID', platform.system())}-"
                f"{os_release.get('VERSION_ID', platform.release())}"
            ),
            "arch": platform.machine(),
        },
    }


def implementation_sha256() -> str:
    digest = hashlib.sha256()
    for relative in IMPLEMENTATION_FILES:
        path = ROOT / relative
        digest.update(relative.encode())
        digest.update(b"\0")
        digest.update(path.read_bytes() if path.is_file() else b"missing")
        digest.update(b"\0")
    return digest.hexdigest()


def file_sha256(path: Path) -> str | None:
    if not path.is_file():
        return None
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def converter_path() -> Path:
    configured = os.environ.get("FILECONV_BIN")
    return Path(configured).expanduser() if configured else DEFAULT_FILECONV


def converter_environment() -> dict[str, str]:
    environment = os.environ.copy()
    pdfium = ROOT / "pdfium/lib"
    if pdfium.is_dir() and "FILECONV_PDFIUM_LIB" not in environment:
        environment["FILECONV_PDFIUM_LIB"] = str(pdfium)
    tessdata = ROOT / "tessdata_best"
    if tessdata.is_dir() and "FILECONV_TESSDATA" not in environment:
        environment["FILECONV_TESSDATA"] = str(tessdata)
    return environment


def converter_version(converter: Path) -> dict:
    if not converter.is_file():
        return {"available": False, "path": str(converter)}
    for args in (["--version"], ["version"]):
        try:
            completed = subprocess.run(
                [str(converter), *args],
                cwd=ROOT,
                text=True,
                capture_output=True,
                timeout=5,
            )
        except (OSError, subprocess.SubprocessError):
            continue
        output = (completed.stdout or completed.stderr).strip()
        if completed.returncode == 0 and output:
            return {
                "available": True,
                "path": str(converter),
                "sha256": file_sha256(converter),
                "versionOutput": output[:500],
            }
    return {
        "available": True,
        "path": str(converter),
        "sha256": file_sha256(converter),
        "versionOutput": "unknown",
    }


def workload_formats(profile: dict) -> list[str]:
    formats = profile.get("workloads", {}).get("ingest", {}).get("formats", [])
    if not formats:
        raise HarnessError("workload-profile.yaml missing workloads.ingest.formats")
    return [str(item) for item in formats]


def select_documents(manifest: dict, formats: list[str]) -> tuple[list[dict], list[dict]]:
    all_docs = manifest.get("documents", [])
    if not isinstance(all_docs, list):
        raise HarnessError("golden manifest missing documents array")
    docs_by_format: dict[str, list[dict]] = {}
    for item in all_docs:
        docs_by_format.setdefault(str(item.get("format")), []).append(item)

    missing_formats = [fmt for fmt in formats if fmt not in docs_by_format]
    if missing_formats:
        raise HarnessError(f"golden manifest missing workload formats: {missing_formats}")

    selected: list[dict] = []
    skipped: list[dict] = []
    for item in all_docs:
        fmt = str(item.get("format"))
        if fmt not in formats:
            skipped.append(
                {
                    "documentId": item.get("id"),
                    "format": fmt,
                    "reason": "format_not_in_ingest_workload",
                }
            )
            continue
        conversion_only = bool(item.get("conversionOnly"))
        non_conversion_only_exists = any(
            not bool(candidate.get("conversionOnly")) for candidate in docs_by_format[fmt]
        )
        if conversion_only and non_conversion_only_exists:
            skipped.append(
                {
                    "documentId": item.get("id"),
                    "format": fmt,
                    "reason": "conversionOnly_edge_case_represented_by_non_conversionOnly_docs",
                }
            )
            continue
        reason = "manifest_workload_document"
        if conversion_only:
            reason = "included_conversionOnly_because_format_has_no_other_fixture"
        selected.append({**item, "_selectionReason": reason})
    selected_formats = {str(item["format"]) for item in selected}
    uncovered_formats = [fmt for fmt in formats if fmt not in selected_formats]
    if uncovered_formats:
        raise HarnessError(f"selection failed to cover workload formats: {uncovered_formats}")
    return selected, skipped


def validate_input_files(documents: list[dict]) -> None:
    missing = []
    for item in documents:
        source = CORPUS / "golden" / str(item["path"])
        if not source.is_file():
            missing.append(f"{item['id']} -> {source.relative_to(ROOT)}")
    if missing:
        sample = "\n- ".join(missing[:20])
        raise HarnessError(
            "golden corpus files missing; regenerate or restore bench/markhand_web/golden/documents:\n"
            f"- {sample}"
        )


def count_pdf_pages_with_pdfinfo(path: Path) -> int | None:
    pdfinfo = shutil.which("pdfinfo")
    if not pdfinfo:
        return None
    try:
        completed = subprocess.run(
            [pdfinfo, str(path)],
            text=True,
            capture_output=True,
            timeout=5,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if completed.returncode != 0:
        return None
    match = re.search(r"^Pages:\s+(\d+)\s*$", completed.stdout, re.MULTILINE)
    return int(match.group(1)) if match else None


def estimate_pages(item: dict) -> dict:
    path = CORPUS / "golden" / str(item["path"])
    fmt = str(item["format"])
    if fmt in {"pdf_native", "pdf_scan"}:
        pdfinfo_pages = count_pdf_pages_with_pdfinfo(path)
        if pdfinfo_pages:
            return {"pages": pdfinfo_pages, "source": "pdfinfo"}
        try:
            data = path.read_bytes()
            pages = len(re.findall(rb"/Type\s*/Page\b", data))
            if pages:
                return {"pages": pages, "source": "pdf_page_tokens"}
        except OSError:
            pass
        return {"pages": None, "source": "unavailable"}
    if fmt == "pptx":
        try:
            with zipfile.ZipFile(path) as archive:
                slides = [
                    name
                    for name in archive.namelist()
                    if re.fullmatch(r"ppt/slides/slide\d+\.xml", name)
                ]
            return {"pages": len(slides) or None, "source": "pptx_slide_count"}
        except (OSError, zipfile.BadZipFile):
            return {"pages": None, "source": "unavailable"}
    if fmt == "xlsx":
        try:
            with zipfile.ZipFile(path) as archive:
                sheets = [
                    name
                    for name in archive.namelist()
                    if re.fullmatch(r"xl/worksheets/sheet\d+\.xml", name)
                ]
            return {"pages": len(sheets) or None, "source": "xlsx_sheet_count"}
        except (OSError, zipfile.BadZipFile):
            return {"pages": None, "source": "unavailable"}
    if fmt in {"docx", "html", "csv", "text_legacy"}:
        return {"pages": 1, "source": "single_fixture_estimate"}
    if fmt == "image_ocr":
        return {"pages": 1, "source": "single_image"}
    if fmt == "audio":
        return {"pages": None, "source": "not_applicable_audio"}
    return {"pages": None, "source": "unknown_format"}


def read_rss_kb(pid: int) -> int:
    status = Path("/proc") / str(pid) / "status"
    try:
        for line in status.read_text(errors="replace").splitlines():
            if line.startswith("VmRSS:"):
                parts = line.split()
                return int(parts[1]) if len(parts) > 1 else 0
    except (OSError, ValueError):
        return 0
    return 0


def child_pids(pid: int) -> list[int]:
    task_dir = Path("/proc") / str(pid) / "task"
    children: list[int] = []
    try:
        tasks = list(task_dir.iterdir())
    except OSError:
        return children
    for task in tasks:
        children_file = task / "children"
        try:
            for value in children_file.read_text().split():
                children.append(int(value))
        except (OSError, ValueError):
            continue
    return children


def process_tree_pids(pid: int) -> list[int]:
    seen: set[int] = set()
    stack = [pid]
    while stack:
        current = stack.pop()
        if current in seen:
            continue
        seen.add(current)
        stack.extend(child for child in child_pids(current) if child not in seen)
    return list(seen)


def process_tree_rss_kb(pid: int) -> int | None:
    proc = Path("/proc")
    if not proc.is_dir():
        return None
    pids = process_tree_pids(pid)
    if not pids:
        return None
    return sum(read_rss_kb(child) for child in pids)


def kill_process_tree(pid: int) -> None:
    pids = sorted(process_tree_pids(pid), reverse=True)
    for child in pids:
        try:
            os.kill(child, signal.SIGKILL)
        except ProcessLookupError:
            pass
        except PermissionError:
            continue


def run_one_conversion(
    converter: Path,
    item: dict,
    run_label: str,
    timeout_seconds: float,
) -> dict:
    source = CORPUS / "golden" / str(item["path"])
    page_estimate = estimate_pages(item)
    started = time.perf_counter()
    start_utc = dt.datetime.now(dt.timezone.utc).isoformat().replace("+00:00", "Z")
    peak_rss_kb: int | None = 0 if Path("/proc").is_dir() else None
    timed_out = False
    try:
        process = subprocess.Popen(
            [str(converter), "one", str(source)],
            cwd=ROOT,
            env=converter_environment(),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
    except OSError as error:
        elapsed_ms = round((time.perf_counter() - started) * 1000, 3)
        return {
            "run": run_label,
            "documentId": item["id"],
            "format": item["format"],
            "path": item["path"],
            "selectionReason": item["_selectionReason"],
            "status": "error",
            "exitCode": None,
            "durationMs": elapsed_ms,
            "pages": page_estimate["pages"],
            "pageEstimateSource": page_estimate["source"],
            "peakRssMb": None,
            "rssMeasured": False,
            "stdoutBytes": 0,
            "stdoutSha256": None,
            "stderrTail": str(error),
            "timedOut": False,
            "startedAt": start_utc,
        }

    while process.poll() is None:
        rss = process_tree_rss_kb(process.pid)
        if rss is not None:
            peak_rss_kb = max(peak_rss_kb or 0, rss)
        if time.perf_counter() - started > timeout_seconds:
            timed_out = True
            kill_process_tree(process.pid)
            break
        time.sleep(0.02)

    try:
        stdout, stderr = process.communicate(timeout=2)
    except subprocess.TimeoutExpired:
        kill_process_tree(process.pid)
        stdout, stderr = process.communicate()
        timed_out = True

    elapsed_ms = round((time.perf_counter() - started) * 1000, 3)
    exit_code = process.returncode
    status = "ok" if exit_code == 0 and not timed_out else "timeout" if timed_out else "error"
    stderr_text = stderr.decode("utf-8", errors="replace").replace(str(ROOT), "<repo>")
    return {
        "run": run_label,
        "documentId": item["id"],
        "format": item["format"],
        "path": item["path"],
        "selectionReason": item["_selectionReason"],
        "status": status,
        "exitCode": exit_code,
        "durationMs": elapsed_ms,
        "pages": page_estimate["pages"],
        "pageEstimateSource": page_estimate["source"],
        "peakRssMb": round((peak_rss_kb or 0) / 1024, 3) if peak_rss_kb is not None else None,
        "rssMeasured": peak_rss_kb is not None,
        "stdoutBytes": len(stdout),
        "stdoutSha256": hashlib.sha256(stdout).hexdigest() if stdout else None,
        "stderrTail": stderr_text[-1200:] if status != "ok" else "",
        "timedOut": timed_out,
        "startedAt": start_utc,
    }


def percentile(values: list[float], pct: float) -> float:
    if not values:
        return 0.0
    ordered = sorted(values)
    if len(ordered) == 1:
        return ordered[0]
    rank = (len(ordered) - 1) * pct
    lower = int(rank)
    upper = min(lower + 1, len(ordered) - 1)
    fraction = rank - lower
    return ordered[lower] + (ordered[upper] - ordered[lower]) * fraction


def duration_stats_ms(values: list[float]) -> dict:
    if not values:
        return {"min": 0.0, "mean": 0.0, "p50": 0.0, "p95": 0.0, "max": 0.0}
    return {
        "min": round(min(values), 3),
        "mean": round(statistics.fmean(values), 3),
        "p50": round(percentile(values, 0.50), 3),
        "p95": round(percentile(values, 0.95), 3),
        "max": round(max(values), 3),
    }


def summarize_run(run_label: str, worker_count: int, results: list[dict], wall_seconds: float) -> dict:
    successes = [item for item in results if item["status"] == "ok"]
    failures = [item for item in results if item["status"] != "ok"]
    wall_hours = wall_seconds / 3600.0 if wall_seconds > 0 else 0.0
    docs_per_hour = len(successes) / wall_hours if wall_hours else 0.0
    estimated_pages = sum(item["pages"] or 0 for item in successes)
    pages_per_hour = estimated_pages / wall_hours if wall_hours else 0.0
    per_format: dict[str, dict] = {}
    for fmt in sorted({str(item["format"]) for item in results}):
        format_results = [item for item in results if item["format"] == fmt]
        format_successes = [item for item in format_results if item["status"] == "ok"]
        duration_values = [float(item["durationMs"]) for item in format_results]
        success_duration_seconds = sum(float(item["durationMs"]) for item in format_successes) / 1000.0
        format_docs_per_hour = (
            len(format_successes) / (success_duration_seconds / 3600.0)
            if success_duration_seconds > 0
            else 0.0
        )
        rss_values = [
            float(item["peakRssMb"])
            for item in format_results
            if item.get("peakRssMb") is not None
        ]
        per_format[fmt] = {
            "documents": len(format_results),
            "successes": len(format_successes),
            "failures": len(format_results) - len(format_successes),
            "docsPerHourFromFileDurations": round(format_docs_per_hour, 3),
            "durationMs": duration_stats_ms(duration_values),
            "peakRssMb": round(max(rss_values), 3) if rss_values else None,
            "pagesEstimated": sum(item["pages"] or 0 for item in format_results),
        }
    rss_values = [float(item["peakRssMb"]) for item in results if item.get("peakRssMb") is not None]
    return {
        "label": run_label,
        "workerCount": worker_count,
        "documentsAttempted": len(results),
        "documentsSucceeded": len(successes),
        "documentsFailed": len(failures),
        "wallSeconds": round(wall_seconds, 3),
        "docsPerHour": round(docs_per_hour, 3),
        "estimatedPagesSucceeded": estimated_pages,
        "pagesPerHour": round(pages_per_hour, 3),
        "durationMs": duration_stats_ms([float(item["durationMs"]) for item in results]),
        "peakRssMb": round(max(rss_values), 3) if rss_values else None,
        "rssMeasured": bool(rss_values),
        "perFormat": per_format,
        "failures": [
            {
                "documentId": item["documentId"],
                "format": item["format"],
                "status": item["status"],
                "exitCode": item["exitCode"],
                "stderrTail": item["stderrTail"],
            }
            for item in failures
        ],
    }


def run_batch(
    converter: Path,
    documents: list[dict],
    worker_count: int,
    run_label: str,
    timeout_seconds: float,
) -> tuple[dict, list[dict]]:
    started = time.perf_counter()
    if worker_count == 1:
        results = [
            run_one_conversion(converter, item, run_label, timeout_seconds)
            for item in documents
        ]
    else:
        with concurrent.futures.ThreadPoolExecutor(max_workers=worker_count) as executor:
            futures = [
                executor.submit(run_one_conversion, converter, item, run_label, timeout_seconds)
                for item in documents
            ]
            results = [future.result() for future in futures]
    wall_seconds = time.perf_counter() - started
    return summarize_run(run_label, worker_count, results, wall_seconds), results


def simulate_queue(
    service_docs_per_hour: float,
    arrival_docs_per_hour: float,
    duration_minutes: float,
) -> dict:
    duration_hours = duration_minutes / 60.0
    arrivals = arrival_docs_per_hour * duration_hours
    service_capacity = service_docs_per_hour * duration_hours
    final_queue = max(0.0, arrivals - service_capacity)
    if arrival_docs_per_hour <= 0:
        age_minutes = 0.0
    elif service_docs_per_hour >= arrival_docs_per_hour:
        age_minutes = 0.0
    else:
        age_minutes = duration_minutes * (1.0 - (service_docs_per_hour / arrival_docs_per_hour))
    return {
        "type": "deterministic_fluid_queue_simulation",
        "arrivalDocsPerHour": round(arrival_docs_per_hour, 3),
        "serviceDocsPerHour": round(service_docs_per_hour, 3),
        "durationMinutes": round(duration_minutes, 3),
        "arrivals": round(arrivals, 3),
        "serviceCapacity": round(service_capacity, 3),
        "finalQueueDocuments": round(final_queue, 3),
        "oldestQueueAgeMinutesAtWindowEnd": round(max(0.0, age_minutes), 3),
        "steadyStateStable": service_docs_per_hour >= arrival_docs_per_hour,
        "unboundedIfSustained": service_docs_per_hour < arrival_docs_per_hour,
    }


def queue_simulations(
    profile: dict,
    measured_service_docs_per_hour: float,
    effective_service_docs_per_hour: float,
    capacity_valid_for_gate: bool,
) -> dict:
    loads = profile["loads"]
    normal = float(loads["normal"]["ingestDocumentsPerHour"])
    peak = float(loads["peak"]["ingestDocumentsPerHour"])
    recovery = loads.get("recovery", {})
    multiplier = float(recovery.get("loadMultiplier", 2.0))
    duration = float(recovery.get("durationMinutes", 120.0))
    return {
        "label": "simulation_from_measured_local_cpu_rate",
        "measuredServiceDocsPerHour": round(measured_service_docs_per_hour, 3),
        "effectiveServiceDocsPerHour": round(effective_service_docs_per_hour, 3),
        "capacityValidForGate": capacity_valid_for_gate,
        "effectiveRateNote": (
            "all workload documents succeeded"
            if capacity_valid_for_gate
            else "set to zero for queue simulation because one or more workload documents failed"
        ),
        "normal1x": simulate_queue(effective_service_docs_per_hour, normal, duration),
        "recovery2xNormal": simulate_queue(effective_service_docs_per_hour, normal * multiplier, duration),
        "peakGateLoad": simulate_queue(effective_service_docs_per_hour, peak, duration),
    }


def headroom_estimate(
    measured_service_docs_per_hour: float,
    effective_service_docs_per_hour: float,
    all_succeeded: bool,
) -> dict:
    required_for_headroom = TARGET_DOCS_PER_HOUR / (1.0 - HEADROOM_TARGET_PERCENT / 100.0)
    if effective_service_docs_per_hour > 0:
        estimated = (1.0 - (TARGET_DOCS_PER_HOUR / effective_service_docs_per_hour)) * 100.0
    else:
        estimated = -100.0
    return {
        "basis": "local-cpu concurrent run docs/hour vs G0-CAP target",
        "targetDocsPerHour": TARGET_DOCS_PER_HOUR,
        "targetHeadroomPercent": HEADROOM_TARGET_PERCENT,
        "requiredDocsPerHourForTargetHeadroom": round(required_for_headroom, 3),
        "measuredSuccessfulDocsPerHour": round(measured_service_docs_per_hour, 3),
        "effectiveCapacityDocsPerHourForGate": round(effective_service_docs_per_hour, 3),
        "capacityValidForGate": bool(all_succeeded),
        "estimatedHeadroomPercent": round(estimated, 3),
        "meetsHeadroomTargetOnThisRunner": bool(all_succeeded and estimated >= HEADROOM_TARGET_PERCENT),
    }


def build_payload(args: argparse.Namespace) -> dict:
    status = git_status()
    profile = load_json(WORKLOAD_PROFILE)
    manifest = load_json(args.manifest)
    formats = workload_formats(profile)
    selected, skipped = select_documents(manifest, formats)
    validate_input_files(selected)
    converter = converter_path()
    if not converter.is_file():
        raise HarnessError(
            f"fileconv binary missing: {converter}. Set FILECONV_BIN or run cargo build --release."
        )

    run_summaries: list[dict] = []
    all_results: dict[str, list[dict]] = {}
    for worker_count, label in ((1, "singleWorker"), (CONCURRENT_WORKERS, "concurrent2")):
        summary, results = run_batch(
            converter=converter,
            documents=selected,
            worker_count=worker_count,
            run_label=label,
            timeout_seconds=args.timeout_seconds,
        )
        run_summaries.append(summary)
        all_results[label] = results

    capacity_run = run_summaries[-1]
    docs_per_hour = float(capacity_run["docsPerHour"])
    all_succeeded = all(summary["documentsFailed"] == 0 for summary in run_summaries)
    effective_docs_per_hour_for_gate = docs_per_hour if all_succeeded else 0.0
    target_match = False
    production_capacity_blocked = True
    profile_b_gate_passed = False
    honest_flags_set = (
        target_match is False
        and production_capacity_blocked is True
        and profile_b_gate_passed is False
        and DOES_NOT_CLAIM
    )
    closure = {
        "harnessCompleted": True,
        "reportWritten": True,
        "gitClean": not status["dirty"],
        "honestFlagsSet": bool(honest_flags_set),
    }
    p0_08_closed = all(closure.values())
    return {
        "version": 1,
        "reportId": "p0-08-ingest-capacity",
        "gateId": "G0-CAP-INGEST-THROUGHPUT",
        "generatedAt": dt.datetime.now(dt.timezone.utc).isoformat().replace("+00:00", "Z"),
        "command": "bash bench/markhand_web/scripts/run_ingest_capacity.sh",
        "mode": "local-cpu-converter-smoke",
        "measurementScope": "local-cpu; not Profile B",
        "git": status,
        "environment": {
            "environmentId": "current-runner-local-cpu",
            "targetEnvironmentId": "on-prem-reference",
            "targetMatch": target_match,
            "fingerprint": {
                "gitCommit": status["commit"],
                "workloadProfileId": profile["profileId"],
                "hardware": hardware_fingerprint(ROOT),
            },
        },
        "implementationSha256": implementation_sha256(),
        "converter": converter_version(converter),
        "workloadProfile": {
            "profileId": profile["profileId"],
            "formats": formats,
            "loads": profile["loads"],
            "headroomPercent": profile["hardware"]["headroomPercent"],
        },
        "manifest": {
            "path": str(args.manifest.relative_to(ROOT) if args.manifest.is_relative_to(ROOT) else args.manifest),
            "sha256": file_sha256(args.manifest),
            "documentsInManifest": len(manifest.get("documents", [])),
            "documentsSelected": len(selected),
            "formatsCovered": sorted({str(item["format"]) for item in selected}),
            "conversionOnlyPolicy": "skip only when a non-conversionOnly fixture covers the same format",
            "skippedDocuments": skipped,
        },
        "runs": run_summaries,
        "results": all_results,
        "docsPerHour": round(docs_per_hour, 3),
        "perFormat": capacity_run["perFormat"],
        "headroomEstimate": headroom_estimate(
            measured_service_docs_per_hour=docs_per_hour,
            effective_service_docs_per_hour=effective_docs_per_hour_for_gate,
            all_succeeded=all_succeeded,
        ),
        "queueAgeSimulation": queue_simulations(
            profile,
            measured_service_docs_per_hour=docs_per_hour,
            effective_service_docs_per_hour=effective_docs_per_hour_for_gate,
            capacity_valid_for_gate=all_succeeded,
        ),
        "allDocumentsSucceeded": all_succeeded,
        "targetMatch": target_match,
        "profileBGatePassed": profile_b_gate_passed,
        "productionCapacityBlocked": production_capacity_blocked,
        "closure": closure,
        "p0_08_closed": p0_08_closed,
        "doesNotClaim": [
            "Profile B evidence",
            "G0-CAP-INGEST-THROUGHPUT pass",
            "production headroom",
        ],
        "notes": [
            "Local-cpu smoke/sizing only; re-run on on-prem-reference Profile B for the gate.",
            "Queue age values are deterministic simulations from measured converter service rate.",
            DOES_NOT_CLAIM,
        ],
    }


def render_report(payload: dict) -> str:
    capacity = payload["runs"][-1]
    headroom = payload["headroomEstimate"]
    queue = payload["queueAgeSimulation"]
    lines = [
        "# P0-08 ingest capacity report",
        "",
        f"- Generated: `{payload['generatedAt']}`",
        f"- Mode: `{payload['mode']}`",
        f"- Measurement scope: `{payload['measurementScope']}`",
        f"- Git commit: `{payload['git']['commit']}`",
        f"- Dirty at harness start: `{str(payload['git']['dirty']).lower()}`",
        f"- `targetMatch`: `{str(payload['targetMatch']).lower()}`",
        f"- `profileBGatePassed`: `{str(payload['profileBGatePassed']).lower()}`",
        f"- `productionCapacityBlocked`: `{str(payload['productionCapacityBlocked']).lower()}`",
        f"- `p0_08_closed`: `{str(payload['p0_08_closed']).lower()}`",
        "",
        "## Scope",
        "",
        "This is local-cpu converter smoke evidence. It does **not** claim Profile B",
        "headroom or a `G0-CAP-INGEST-THROUGHPUT` pass.",
        "",
        f"Explicit note: {DOES_NOT_CLAIM}.",
        "",
        "## Workload coverage",
        "",
        f"- Manifest documents: `{payload['manifest']['documentsInManifest']}`",
        f"- Selected documents: `{payload['manifest']['documentsSelected']}`",
        f"- Formats covered: `{', '.join(payload['manifest']['formatsCovered'])}`",
        f"- Conversion-only policy: {payload['manifest']['conversionOnlyPolicy']}.",
        "",
        "## Runs",
        "",
        "| run | workers | docs ok/error | wall s | docs/hour | pages/hour | peak RSS MB |",
        "|---|---:|---:|---:|---:|---:|---:|",
    ]
    for run in payload["runs"]:
        lines.append(
            "| "
            f"{run['label']} | {run['workerCount']} | "
            f"{run['documentsSucceeded']}/{run['documentsFailed']} | "
            f"{run['wallSeconds']} | {run['docsPerHour']} | "
            f"{run['pagesPerHour']} | {run['peakRssMb']} |"
        )
    lines.extend(
        [
            "",
            "## Per-format sizing from concurrent run",
            "",
            "| format | docs | ok | failed | docs/hour from file durations | p95 ms | peak RSS MB | pages est. |",
            "|---|---:|---:|---:|---:|---:|---:|---:|",
        ]
    )
    for fmt, stats in capacity["perFormat"].items():
        lines.append(
            "| "
            f"{fmt} | {stats['documents']} | {stats['successes']} | "
            f"{stats['failures']} | {stats['docsPerHourFromFileDurations']} | "
            f"{stats['durationMs']['p95']} | {stats['peakRssMb']} | "
            f"{stats['pagesEstimated']} |"
        )
    lines.extend(
        [
            "",
            "## Headroom estimate",
            "",
            f"- Target: `{headroom['targetDocsPerHour']}` docs/hour.",
            f"- Required for 30% headroom: `{headroom['requiredDocsPerHourForTargetHeadroom']}` docs/hour.",
            f"- Measured successful local-cpu throughput: `{headroom['measuredSuccessfulDocsPerHour']}` docs/hour.",
            f"- Gate-valid effective capacity: `{headroom['effectiveCapacityDocsPerHourForGate']}` docs/hour.",
            f"- Estimated headroom: `{headroom['estimatedHeadroomPercent']}`%.",
            f"- Meets 30% headroom on this runner: `{str(headroom['meetsHeadroomTargetOnThisRunner']).lower()}`.",
            "",
            "## Queue age simulation",
            "",
            "These rows are deterministic simulations. If any workload format failed,",
            "the gate-valid service rate is set to zero instead of extrapolating from",
            "partial successes.",
            "",
            f"- Measured service rate: `{queue['measuredServiceDocsPerHour']}` docs/hour.",
            f"- Effective simulated service rate: `{queue['effectiveServiceDocsPerHour']}` docs/hour.",
            f"- Capacity valid for gate: `{str(queue['capacityValidForGate']).lower()}`.",
            f"- Note: {queue['effectiveRateNote']}.",
            "",
            "| scenario | arrival docs/hour | final queue docs | oldest age min | stable |",
            "|---|---:|---:|---:|---|",
        ]
    )
    for name in ("normal1x", "recovery2xNormal", "peakGateLoad"):
        sim = queue[name]
        lines.append(
            "| "
            f"{name} | {sim['arrivalDocsPerHour']} | "
            f"{sim['finalQueueDocuments']} | {sim['oldestQueueAgeMinutesAtWindowEnd']} | "
            f"{str(sim['steadyStateStable']).lower()} |"
        )
    lines.extend(
        [
            "",
            "## Closure",
            "",
            "| field | value |",
            "|---|---|",
        ]
    )
    for key, value in payload["closure"].items():
        lines.append(f"| `{key}` | `{str(value).lower()}` |")
    failures = []
    for run in payload["runs"]:
        failures.extend({**failure, "run": run["label"]} for failure in run["failures"])
    if failures:
        lines.extend(["", "## Failures/timeouts", ""])
        for failure in failures[:20]:
            lines.append(
                f"- `{failure['run']}` `{failure['documentId']}` "
                f"({failure['format']}): {failure['status']} exit={failure['exitCode']}"
            )
    if payload["git"]["dirtyPaths"]:
        lines.extend(["", "Dirty paths at harness start:"])
        for path in payload["git"]["dirtyPaths"][:20]:
            lines.append(f"- `{path}`")
    lines.append("")
    return "\n".join(lines)


def write_outputs(payload: dict, summary: Path, report: Path) -> None:
    summary.parent.mkdir(parents=True, exist_ok=True)
    report.parent.mkdir(parents=True, exist_ok=True)
    summary.write_text(json.dumps(payload, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    report.write_text(render_report(payload), encoding="utf-8")


def self_test() -> None:
    profile = {
        "loads": {
            "normal": {"ingestDocumentsPerHour": 300},
            "peak": {"ingestDocumentsPerHour": 1200},
            "recovery": {"loadMultiplier": 2.0, "durationMinutes": 120},
        }
    }
    stable = simulate_queue(900, 600, 120)
    assert stable["steadyStateStable"] is True
    assert stable["oldestQueueAgeMinutesAtWindowEnd"] == 0.0
    unstable = simulate_queue(300, 600, 120)
    assert unstable["steadyStateStable"] is False
    assert unstable["finalQueueDocuments"] == 600.0
    assert unstable["oldestQueueAgeMinutesAtWindowEnd"] == 60.0
    headroom = headroom_estimate(1800, 1800, True)
    assert headroom["meetsHeadroomTargetOnThisRunner"] is True
    invalid_headroom = headroom_estimate(1800, 0, False)
    assert invalid_headroom["meetsHeadroomTargetOnThisRunner"] is False
    queue = queue_simulations(profile, 300, 300, True)
    assert queue["recovery2xNormal"]["unboundedIfSustained"] is True
    invalid_queue = queue_simulations(profile, 1800, 0, False)
    assert invalid_queue["normal1x"]["finalQueueDocuments"] == 600.0
    manifest = {
        "documents": [
            {"id": "doc", "path": "documents/doc.docx", "format": "docx", "conversionOnly": False},
            {"id": "audio", "path": "documents/audio.wav", "format": "audio", "conversionOnly": True},
        ]
    }
    selected, skipped = select_documents(manifest, ["docx", "audio"])
    assert len(selected) == 2
    assert skipped == []


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=MANIFEST_PATH)
    parser.add_argument("--summary", type=Path, default=SUMMARY_PATH)
    parser.add_argument("--report", type=Path, default=REPORT_PATH)
    parser.add_argument("--timeout-seconds", type=float, default=CONVERSION_TIMEOUT_SECONDS)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    args.manifest = args.manifest.resolve()
    args.summary = args.summary.resolve()
    args.report = args.report.resolve()
    if args.self_test:
        self_test()
        print("self-test ok")
        return 0
    try:
        payload = build_payload(args)
        write_outputs(payload, args.summary, args.report)
    except HarnessError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    print(f"wrote {args.summary.relative_to(ROOT)}")
    print(f"wrote {args.report.relative_to(ROOT)}")
    print(f"docsPerHour={payload['docsPerHour']}")
    print(f"p0_08_closed={str(payload['p0_08_closed']).lower()}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
