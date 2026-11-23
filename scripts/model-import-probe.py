"""Offline real-weight installation probe. No microphone, app settings or inference."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

from voiceweave_worker.model_install import (
    EXPECTED_FILES,
    InstallCancelled,
    REVISION,
    import_standard,
    verify_model_dir,
)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--temporary-parent", type=Path,
                        help="Existing directory with at least 1.64 GB free for the test copy")
    args = parser.parse_args()
    source = args.source.absolute()
    before = {name: (source / name).stat().st_mtime_ns for name in EXPECTED_FILES}
    started = time.perf_counter()
    verify_model_dir(source)
    verified_ms = round((time.perf_counter() - started) * 1000)
    events = []
    with tempfile.TemporaryDirectory(prefix="voiceweave-import-probe-",
                                     dir=args.temporary_parent) as temporary:
        root = Path(temporary)
        cancelled = False

        def cancel_on_copy(event):
            nonlocal cancelled
            if event.phase == "copying" and event.completed_bytes > 0:
                cancelled = True

        try:
            import_standard(source, root, progress=cancel_on_copy,
                            cancelled=lambda: cancelled)
        except InstallCancelled:
            pass
        else:
            raise RuntimeError("cancellation failed")
        destination = root / "standard" / REVISION
        if destination.exists() or list((root / "standard").glob(".install-*")):
            raise RuntimeError("cancelled staging was not removed")
        started = time.perf_counter()
        installed = import_standard(source, root, progress=events.append)
        imported_ms = round((time.perf_counter() - started) * 1000)
        if installed != destination or not events or events[-1].phase != "installed":
            raise RuntimeError("installation did not finish")
        before_repeat = (installed / "model.bin").stat().st_mtime_ns
        # Exercise the public CLI. Missing source is harmless for an existing valid install.
        env = dict(os.environ, HF_HUB_OFFLINE="1")
        result = subprocess.run(
            [sys.executable, "-m", "voiceweave_worker.model_install", "--model-root",
             str(root), "--source", str(root / "absent")],
            env=env, capture_output=True, timeout=180,
        )
        if result.returncode != 0:
            raise RuntimeError("CLI revalidation failed")
        if (installed / "model.bin").stat().st_mtime_ns != before_repeat:
            raise RuntimeError("idempotent import rewrote the weights")
        for phase in ("copying", "verifying"):
            counts = [event.completed_bytes for event in events if event.phase == phase]
            if counts != sorted(counts) or counts[-1] != sum(size for size, _ in EXPECTED_FILES.values()):
                raise RuntimeError("invalid progress counts")
    after = {name: (source / name).stat().st_mtime_ns for name in EXPECTED_FILES}
    if before != after:
        raise RuntimeError("source modification times changed")
    print(json.dumps({"ok": True, "source_verify_ms": verified_ms,
                      "copy_and_verify_ms": imported_ms,
                      "bytes": sum(size for size, _ in EXPECTED_FILES.values()),
                      "cancel_cleanup": True, "cli_revalidation": True,
                      "source_mtime_unchanged": True}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
