"""Optional GPU runtime: the two CUDA 12 cuBLAS DLLs, from a pinned PyPI wheel.

Explicit, user-triggered, never run by the recognition worker. The wheel and each
extracted DLL are checked against the packaged catalog before anything is published.
Layout: <root>/cuda/{cublas64_12.dll,cublasLt64_12.dll,install-manifest.json}
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import tempfile
import urllib.request
import zipfile
from importlib.resources import files
from pathlib import Path
from urllib.parse import urlparse

from voiceweave_worker.model_install import (
    Cancelled, InstallBusy, InstallCancelled, Progress, _check_cancel, _emit, _install_lock,
    _reject_links, _sha256,
)

__all__ = ["InstallBusy", "InstallCancelled", "install_cuda", "verify_cuda_dir"]

_DOWNLOAD_HOST = "files.pythonhosted.org"


def _load_catalog(path: Path) -> dict:
    catalog = json.loads(path.read_text(encoding="utf-8"))
    wheel = catalog["wheel"]
    url = urlparse(wheel["url"])
    if (type(catalog["schema_version"]) is not int or catalog["schema_version"] != 1
            or catalog["package"] != "nvidia-cublas-cu12"
            or not re.fullmatch(r"\d+(\.\d+)+", catalog["version"])
            or url.scheme != "https" or url.hostname != _DOWNLOAD_HOST
            or not re.fullmatch(r"[a-f0-9]{64}", wheel["sha256"])
            or type(wheel["bytes"]) is not int or wheel["bytes"] <= 0):
        raise ValueError("invalid packaged CUDA catalog")
    names = [member["name"] for member in catalog["members"]]
    if sorted(names) != ["cublas64_12.dll", "cublasLt64_12.dll"]:
        raise ValueError("invalid CUDA catalog members")
    for member in catalog["members"]:
        if (type(member["bytes"]) is not int or member["bytes"] <= 0
                or not re.fullmatch(r"[a-f0-9]{64}", member["sha256"])
                or not re.fullmatch(r"[a-zA-Z0-9_/.-]+", member["wheel_path"])
                or ".." in member["wheel_path"].split("/")):
            raise ValueError("invalid CUDA catalog member")
    return catalog


CATALOG = _load_catalog(files("voiceweave_worker").joinpath("cuda_catalog.json"))


def verify_cuda_dir(directory: Path, *, catalog: dict | None = None,
                    progress: Progress = None, cancelled: Cancelled = None) -> None:
    """Reject a missing, resized or modified runtime before it is marked usable."""
    catalog = catalog or CATALOG
    _check_cancel(cancelled)
    _reject_links(directory.absolute())
    if directory.is_symlink() or not directory.is_dir():
        raise ValueError("CUDA directory is missing or invalid")
    total = sum(member["bytes"] for member in catalog["members"])
    done = 0
    _emit(progress, "verifying", done, total)

    def on_block(count: int) -> None:
        nonlocal done
        done += count
        _emit(progress, "verifying", done, total)

    for member in catalog["members"]:
        path = directory / member["name"]
        _reject_links(path.absolute())
        if (not path.is_file() or path.is_symlink()
                or path.stat().st_size != member["bytes"]
                or _sha256(path, cancelled=cancelled, on_block=on_block) != member["sha256"]):
            raise ValueError(f"{member['name']} does not match the pinned release")


def _download_wheel(destination: Path, catalog: dict, progress: Progress,
                    cancelled: Cancelled) -> None:
    wheel = catalog["wheel"]
    request = urllib.request.Request(wheel["url"], headers={"User-Agent": "VoiceWeave-cuda-install"})
    received = 0
    _emit(progress, "downloading", 0, wheel["bytes"])
    with urllib.request.urlopen(request, timeout=60) as response, destination.open("xb") as out:
        if urlparse(response.geturl()).hostname != _DOWNLOAD_HOST:
            raise ValueError("unexpected download redirect")
        while True:
            _check_cancel(cancelled)
            block = response.read(1024 * 1024)
            if not block:
                break
            received += len(block)
            if received > wheel["bytes"]:
                raise ValueError("downloaded wheel is larger than the pinned release")
            out.write(block)
            _emit(progress, "downloading", received, wheel["bytes"])


def _extract_members(wheel_path: Path, staging: Path, catalog: dict, progress: Progress,
                     cancelled: Cancelled) -> None:
    """Copy exactly the catalogued members by exact name; every other entry is ignored."""
    total = sum(member["bytes"] for member in catalog["members"])
    done = 0
    _emit(progress, "extracting", done, total)
    with zipfile.ZipFile(wheel_path) as archive:
        entries = {info.filename: info for info in archive.infolist()}
        for member in catalog["members"]:
            info = entries.get(member["wheel_path"])
            if info is None or info.file_size != member["bytes"] or info.is_dir():
                raise ValueError(f"{member['name']} is missing from the pinned wheel")
            remaining = member["bytes"]
            with archive.open(info) as reader, (staging / member["name"]).open("xb") as writer:
                while remaining:
                    _check_cancel(cancelled)
                    block = reader.read(min(1024 * 1024, remaining))
                    if not block:
                        raise ValueError("wheel member ended early")
                    writer.write(block)
                    remaining -= len(block)
                    done += len(block)
                    _emit(progress, "extracting", done, total)
                if reader.read(1):
                    raise ValueError("wheel member is larger than catalogued")


def install_cuda(root: Path, *, wheel: Path | None = None, progress: Progress = None,
                 cancelled: Cancelled = None, catalog: dict | None = None) -> Path:
    """Publish <root>/cuda after verifying a private staging copy. Never replaces anything.

    `wheel` supplies an already downloaded copy (still hash-checked); otherwise the pinned
    URL is fetched. An existing install is verified and returned, never overwritten.
    """
    catalog = catalog or CATALOG
    _check_cancel(cancelled)
    _reject_links(root.absolute())
    root.mkdir(parents=True, exist_ok=True)
    root = root.resolve(strict=True)
    destination = root / "cuda"
    with _install_lock(root):
        _reject_links(destination)
        if destination.exists():
            verify_cuda_dir(destination, catalog=catalog, progress=progress, cancelled=cancelled)
            _emit(progress, "installed")
            return destination
        extracted = sum(member["bytes"] for member in catalog["members"])
        need = extracted + (0 if wheel else catalog["wheel"]["bytes"]) + 64 * 1024 * 1024
        if shutil.disk_usage(root).free < need:
            raise OSError("insufficient disk space for the CUDA runtime")
        staging = Path(tempfile.mkdtemp(prefix=".install-cuda-", dir=root))
        try:
            source = wheel
            if source is None:
                source = staging / "wheel.part"
                _download_wheel(source, catalog, progress, cancelled)
            _reject_links(source.absolute())
            if not source.is_file() or source.stat().st_size != catalog["wheel"]["bytes"]:
                raise ValueError("wheel does not match the pinned release")
            total = catalog["wheel"]["bytes"]
            seen = 0

            def on_block(count: int) -> None:
                nonlocal seen
                seen += count
                _emit(progress, "checking-wheel", seen, total)

            if _sha256(source, cancelled=cancelled, on_block=on_block) != catalog["wheel"]["sha256"]:
                raise ValueError("wheel does not match the pinned release")
            runtime = staging / "cuda"
            runtime.mkdir()
            _extract_members(source, runtime, catalog, progress, cancelled)
            verify_cuda_dir(runtime, catalog=catalog, progress=progress, cancelled=cancelled)
            (runtime / "install-manifest.json").write_text(
                json.dumps({"package": catalog["package"], "version": catalog["version"],
                            "license": catalog["license"],
                            "files": {m["name"]: m["sha256"] for m in catalog["members"]}},
                           indent=2) + "\n", encoding="utf-8")
            _check_cancel(cancelled)
            _reject_links(destination)
            runtime.rename(destination)
        finally:
            if staging.exists():
                _reject_links(staging)
                shutil.rmtree(staging.resolve(strict=True))
        _emit(progress, "installed")
    return destination


def _wait_for_cancel_input() -> None:
    """Block until the parent sends "cancel" or closes stdin (EOF).

    A raw descriptor read, not sys.stdin.buffer: a daemon thread blocked inside the
    buffered reader makes the interpreter abort while finalizing after a normal exit.
    Stray bytes (for example a byte-order mark) are ignored.
    """
    received = b""
    while b"cancel" not in received:
        try:
            chunk = os.read(0, 4096)
        except OSError:
            return
        if not chunk:
            return
        received = (received + chunk)[-64:]


def _json_progress(out) -> Progress:
    """One JSON object per line with only a phase and byte counts; at most ~100 per phase."""
    last: dict[str, int] = {}

    def report(progress) -> None:
        total = progress.total_bytes or 0
        step = (progress.completed_bytes * 100 // total) if total else 0
        if last.get(progress.phase) == step and progress.completed_bytes != total:
            return
        last[progress.phase] = step
        out.write(json.dumps({"phase": progress.phase, "completed": progress.completed_bytes,
                              "total": total}) + chr(10))
        out.flush()

    return report


def main() -> int:
    import sys
    import threading

    parser = argparse.ArgumentParser(description="Install the pinned CUDA 12 cuBLAS runtime")
    parser.add_argument("--root", type=Path, required=True,
                        help="app data root; the runtime is published to <root>/cuda")
    parser.add_argument("--wheel", type=Path,
                        help="use an already downloaded pinned wheel instead of downloading")
    parser.add_argument("--json-progress", action="store_true",
                        help="machine-readable progress; a \"cancel\" line or stdin EOF cancels")
    args = parser.parse_args()
    progress: Progress = None
    cancelled: Cancelled = None
    if args.json_progress:
        stop = threading.Event()

        def watch_stdin() -> None:
            # A cancel request, or the parent disappearing (EOF), stops the install.
            _wait_for_cancel_input()
            stop.set()

        threading.Thread(target=watch_stdin, daemon=True).start()
        progress, cancelled = _json_progress(sys.stdout), stop.is_set
    try:
        installed = install_cuda(args.root, wheel=args.wheel, progress=progress,
                                 cancelled=cancelled)
    except InstallCancelled:
        if args.json_progress:
            print(json.dumps({"phase": "cancelled"}), flush=True)
        return 3
    except InstallBusy:
        if args.json_progress:
            print(json.dumps({"phase": "failed", "code": "busy"}), flush=True)
        return 1
    except (OSError, ValueError, RuntimeError, zipfile.BadZipFile) as error:
        if args.json_progress:
            # Only a fixed code: messages can contain local paths.
            print(json.dumps({"phase": "failed", "code": "install_failed"}), flush=True)
            return 1
        parser.exit(1, f"CUDA install failed: {error}" + chr(10))
    if not args.json_progress:
        print(f"verified CUDA directory: {installed}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
