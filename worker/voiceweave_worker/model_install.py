"""Explicit, pinned model download for development. The recognition worker stays offline."""

from __future__ import annotations

import argparse
from contextlib import contextmanager
from dataclasses import dataclass
import hashlib
from importlib.resources import files
import json
import os
import re
import shutil
import stat
import tempfile
from pathlib import Path
from typing import Callable, Iterator, Mapping

# The files each preset's catalog must list, and the licenses accepted for it.
WHISPER_FILES = {"config.json", "model.bin", "preprocessor_config.json",
                 "tokenizer.json", "vocabulary.json"}
REQUIRED_FILES = {
    "standard": WHISPER_FILES,
    "accurate": WHISPER_FILES,
    # NVIDIA Parakeet as published by sherpa-onnx (CC-BY-4.0, NVIDIA).
    "parakeet-ja": {"model.int8.onnx", "tokens.txt"},
    "parakeet-en": {"encoder.int8.onnx", "decoder.int8.onnx", "joiner.int8.onnx", "tokens.txt"},
}
LICENSES = {"standard": "mit", "accurate": "mit", "parakeet-ja": "cc-by-4.0",
            "parakeet-en": "cc-by-4.0"}


def _load_catalog(path: Path, preset: str = "standard") -> dict:
    catalog = json.loads(path.read_text(encoding="utf-8"))
    if (type(catalog["schema_version"]) is not int or catalog["schema_version"] != 1
            or catalog["preset"] != preset
            or not re.fullmatch(r"[a-zA-Z0-9_.-]+/[a-zA-Z0-9_.-]+", catalog["repo_id"])
            or not re.fullmatch(r"[a-f0-9]{40}", catalog["revision"])
            or catalog["license"] != LICENSES.get(preset)):
        raise ValueError("invalid packaged model catalog")
    names = [file["name"] for file in catalog["files"]]
    required = REQUIRED_FILES[preset]
    if len(names) != len(required) or set(names) != required:
        raise ValueError("invalid model catalog file names")
    for file in catalog["files"]:
        if (type(file["bytes"]) is not int or file["bytes"] <= 0
                or not re.fullmatch(r"[a-f0-9]{64}", file["sha256"])):
            raise ValueError("invalid model catalog file metadata")
    return catalog


# This resource is also embedded in Rust at compile time, never read from model input.
CATALOG = _load_catalog(files("voiceweave_worker").joinpath("model_catalog.json"))
REPO_ID = CATALOG["repo_id"]
REVISION = CATALOG["revision"]
EXPECTED_FILES = {file["name"]: (file["bytes"], file["sha256"]) for file in CATALOG["files"]}
# Optional presets, each pinned like the standard one. "accurate" is large-v3 (not turbo):
# more accurate in the 2026-10-06 evaluation, about twice the recognition time.
# Parakeet: Japanese (a main recognizer, on trial) and English (the English shortcut when
# the main recognizer is Parakeet), installed from the settings screen like "accurate".
OPTIONAL_CATALOGS = {
    "accurate": _load_catalog(files("voiceweave_worker").joinpath("model_catalog_accurate.json"),
                              "accurate"),
    "parakeet-ja": _load_catalog(
        files("voiceweave_worker").joinpath("model_catalog_parakeet_ja.json"), "parakeet-ja"),
    "parakeet-en": _load_catalog(
        files("voiceweave_worker").joinpath("model_catalog_parakeet_en.json"), "parakeet-en"),
}
PRESETS = ("standard", *OPTIONAL_CATALOGS)


def _pinned(preset: str) -> tuple[str, str, str, Mapping[str, tuple[int, str]]]:
    """(repo, revision, license, expected files). The standard values are read from the
    module globals at call time so tests can substitute them."""
    if preset == "standard":
        return REPO_ID, REVISION, CATALOG["license"], EXPECTED_FILES
    catalog = OPTIONAL_CATALOGS[preset]
    expected = {file["name"]: (file["bytes"], file["sha256"]) for file in catalog["files"]}
    return catalog["repo_id"], catalog["revision"], catalog["license"], expected


class InstallBusy(RuntimeError):
    """Another process is installing into this model root."""


class InstallCancelled(RuntimeError):
    """Explicit cancellation before publication; no installed model was changed."""


@dataclass(frozen=True)
class InstallProgress:
    # Byte counts are phase-local, not a fabricated overall download percentage.
    phase: str
    completed_bytes: int = 0
    total_bytes: int | None = None


Progress = Callable[[InstallProgress], None] | None
Cancelled = Callable[[], bool] | None


def _emit(progress: Progress, phase: str, completed: int = 0, total: int | None = None) -> None:
    if progress is not None:
        try:
            progress(InstallProgress(phase, completed, total))
        except Exception:
            # A disconnected observer must not turn a successful rename into failure.
            # Cancellation has its own explicit channel, not an observer exception.
            pass


def _check_cancel(cancelled: Cancelled) -> None:
    if cancelled is not None and cancelled():
        raise InstallCancelled("model installation cancelled")


def _reject_links(path: Path) -> None:
    """Refuse Windows junctions/reparse points as well as ordinary symlinks."""
    for component in (path, *path.parents):
        try:
            metadata = component.lstat()
        except FileNotFoundError:
            continue
        if stat.S_ISLNK(metadata.st_mode) or (
            getattr(metadata, "st_file_attributes", 0) & stat.FILE_ATTRIBUTE_REPARSE_POINT
        ):
            raise ValueError("model paths must not contain links or reparse points")


def _sha256(path: Path, *, cancelled: Cancelled = None,
            on_block: Callable[[int], None] | None = None) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            _check_cancel(cancelled)
            digest.update(block)
            if on_block is not None:
                on_block(len(block))
    return digest.hexdigest()


def verify_model_dir(
    directory: Path, *, expected_files: Mapping[str, tuple[int, str]] | None = None,
    progress: Progress = None, cancelled: Cancelled = None,
) -> dict[str, dict[str, str | int]]:
    """Reject incomplete or changed model weights before marking an install usable."""
    if expected_files is None:
        expected_files = EXPECTED_FILES
    _check_cancel(cancelled)
    _reject_links(directory.absolute())
    if directory.is_symlink() or not directory.is_dir():
        raise ValueError("model directory is missing or invalid")
    total = sum(size for size, _ in expected_files.values())
    completed = 0
    _emit(progress, "verifying", completed, total)

    def on_block(count: int) -> None:
        nonlocal completed
        completed += count
        _emit(progress, "verifying", completed, total)

    files: dict[str, dict[str, str | int]] = {}
    for name, (expected_bytes, expected_sha256) in expected_files.items():
        _check_cancel(cancelled)
        path = directory / name
        _reject_links(path.absolute())
        if not path.is_file() or path.is_symlink():
            raise ValueError(f"missing or invalid model file: {name}")
        size = path.stat().st_size
        if size == 0 or size != expected_bytes:
            raise ValueError(f"{name} does not match the pinned release")
        digest = _sha256(path, cancelled=cancelled, on_block=on_block)
        if digest != expected_sha256:
            raise ValueError(f"{name} does not match the pinned release")
        if name.endswith(".json"):
            with path.open("r", encoding="utf-8") as source:
                content = json.load(source)
                expected_type = list if name == "vocabulary.json" else dict
                if not isinstance(content, expected_type) or not content:
                    raise ValueError(f"invalid model JSON: {name}")
        files[name] = {"bytes": size, "sha256": digest}
    _check_cancel(cancelled)
    return files


@contextmanager
def _install_lock(parent: Path) -> Iterator[None]:
    # Keep the file permanently: deleting it after unlock permits two lock inodes.
    # OS locks release when a crashed process closes its file handles.
    import msvcrt

    lock_path = parent / ".install.lock"
    _reject_links(lock_path)
    with lock_path.open("a+b") as lock:
        lock.seek(0)
        try:
            msvcrt.locking(lock.fileno(), msvcrt.LK_NBLCK, 1)
        except OSError as error:
            raise InstallBusy("another model installation is active") from error
        try:
            yield
        finally:
            lock.seek(0)
            msvcrt.locking(lock.fileno(), msvcrt.LK_UNLCK, 1)


def _remove_staging(staging: Path, parent: Path) -> None:
    if not staging.exists():
        return
    # This is only the directory created by this invocation, never a wildcard scan.
    _reject_links(staging)
    resolved = staging.resolve(strict=True)
    if resolved.parent != parent.resolve(strict=True) or not staging.name.startswith(".install-"):
        raise ValueError("unsafe staging cleanup path")
    shutil.rmtree(resolved)


def _install(
    model_root: Path, populate: Callable[[Path], None], *, progress: Progress = None,
    cancelled: Cancelled = None,
    expected_files: Mapping[str, tuple[int, str]] | None = None, preset: str = "standard",
) -> Path:
    """One writer; publication happens only after verifying a private staging copy."""
    repo_id, revision, license_name, pinned_files = _pinned(preset)
    if expected_files is None:
        expected_files = pinned_files
    _check_cancel(cancelled)
    _reject_links(model_root.absolute())
    parent = model_root.absolute() / preset
    _reject_links(parent)
    parent.mkdir(parents=True, exist_ok=True)
    parent = parent.resolve(strict=True)
    destination = parent / revision
    with _install_lock(parent):
        _reject_links(destination)
        if destination.exists():
            # A corrupt existing install is never silently overwritten or removed.
            verify_model_dir(destination, expected_files=expected_files,
                             progress=progress, cancelled=cancelled)
            _emit(progress, "installed")
            return destination
        # The installed release must fit even when the downloader uses sparse/cache
        # files. Space can still change afterwards; write failures abort publication.
        required_bytes = sum(size for size, _ in expected_files.values()) + 16 * 1024 * 1024
        if shutil.disk_usage(parent).free < required_bytes:
            raise OSError("insufficient disk space for the model installation")
        staging = Path(tempfile.mkdtemp(prefix=".install-", dir=parent))
        try:
            populate(staging)
            files = verify_model_dir(staging, expected_files=expected_files,
                                     progress=progress, cancelled=cancelled)
            (staging / "install-manifest.json").write_text(
                json.dumps({"preset": preset, "repo_id": repo_id,
                            "revision": revision, "license": license_name, "files": files},
                           ensure_ascii=False, indent=2) + "\n",
                encoding="utf-8",
            )
            _reject_links(staging / ".cache")
            cache = staging / ".cache"
            if cache.is_dir():
                shutil.rmtree(cache)
            _check_cancel(cancelled)
            _reject_links(destination)
            # Windows rename rejects even an empty existing destination. Do not replace.
            staging.rename(destination)
        finally:
            _remove_staging(staging, parent)
        _emit(progress, "installed")
    return destination


def install_standard(model_root: Path, *, progress: Progress = None,
                     cancelled: Cancelled = None, preset: str = "standard") -> Path:
    """Explicit download only. The download itself cannot be interrupted; a cancel made
    meanwhile takes effect right after it, before anything is verified or published."""
    repo_id, revision, _, expected = _pinned(preset)

    def populate(staging: Path) -> None:
        # Import only after checking existing installation under the process lock.
        from huggingface_hub import snapshot_download

        _emit(progress, "downloading")
        snapshot_download(repo_id=repo_id, revision=revision, local_dir=staging,
                          allow_patterns=[*expected, "README.md"], max_workers=2)
        # A cancel pressed during the download is honored now, not after the checking.
        _check_cancel(cancelled)

    return _install(model_root, populate, progress=progress, cancelled=cancelled, preset=preset)


def import_standard(source: Path, model_root: Path, *, progress: Progress = None,
                    cancelled: Cancelled = None, preset: str = "standard") -> Path:
    """Copy a locally available pinned model; never move the source or use the network."""
    expected = _pinned(preset)[3]

    def populate(staging: Path) -> None:
        _reject_links(source.absolute())
        if not source.is_dir():
            raise ValueError("model source directory is missing")
        total = sum(size for size, _ in expected.values())
        completed = 0
        _emit(progress, "copying", completed, total)
        for name, (expected_size, _) in expected.items():
            _check_cancel(cancelled)
            path = source / name
            _reject_links(path.absolute())
            if not path.is_file() or path.stat().st_size != expected_size:
                raise ValueError(f"{name} does not match the pinned release")
            with path.open("rb") as reader, (staging / name).open("xb") as writer:
                remaining = expected_size
                while remaining:
                    _check_cancel(cancelled)
                    block = reader.read(min(1024 * 1024, remaining))
                    if not block:
                        raise ValueError("model source changed during copy")
                    writer.write(block)
                    remaining -= len(block)
                    completed += len(block)
                    _emit(progress, "copying", completed, total)
                if reader.read(1):
                    raise ValueError("model source changed during copy")
                writer.flush()
                os.fsync(writer.fileno())

    return _install(model_root, populate, progress=progress, cancelled=cancelled, preset=preset)


def main() -> int:
    import sys
    import threading

    parser = argparse.ArgumentParser(description="Install the pinned standard model locally")
    parser.add_argument("--model-root", type=Path, required=True)
    parser.add_argument("--preset", choices=PRESETS, default="standard")
    parser.add_argument("--source", type=Path,
                        help="Copy an existing standard model locally instead of downloading")
    parser.add_argument("--json-progress", action="store_true",
                        help='machine-readable progress; a "cancel" line or stdin EOF cancels')
    args = parser.parse_args()
    progress: Progress = None
    cancelled: Cancelled = None
    if args.json_progress:
        # Shared with the CUDA installer; imported lazily to avoid an import cycle.
        from voiceweave_worker import cuda_install

        stop = threading.Event()

        def watch_stdin() -> None:
            cuda_install._wait_for_cancel_input()
            stop.set()

        threading.Thread(target=watch_stdin, daemon=True).start()
        progress, cancelled = cuda_install._json_progress(sys.stdout), stop.is_set

    def report(phase: str, **fields: object) -> None:
        if args.json_progress:
            print(json.dumps({"phase": phase, **fields}), flush=True)

    try:
        if args.source is not None:
            installed = import_standard(args.source, args.model_root, progress=progress,
                                        cancelled=cancelled, preset=args.preset)
        else:
            # The download itself cannot be interrupted yet; a cancel is honored as soon as
            # it ends, before publishing. Once published, the result is reported as
            # installed, never as cancelled.
            installed = install_standard(args.model_root, progress=progress,
                                         cancelled=cancelled, preset=args.preset)
    except InstallCancelled:
        report("cancelled")
        return 3
    except InstallBusy:
        report("failed", code="busy")
        return 1
    except (OSError, ValueError, RuntimeError) as error:
        if args.json_progress:
            # Only a fixed code: messages can contain local paths.
            report("failed", code="install_failed")
            return 1
        parser.exit(1, f"model install failed: {error}" + chr(10))
    if not args.json_progress:
        print(f"verified model directory: {installed}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
