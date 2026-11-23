"""Model installation faults use small synthetic files, never speech or downloads."""
import hashlib
import json
import os
from pathlib import Path
import queue
import shutil
import subprocess
import sys
import threading
import types

import pytest

from voiceweave_worker import model_install as installer


@pytest.fixture
def model(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> tuple[Path, Path]:
    source = tmp_path / "source"
    source.mkdir()
    for name in installer.EXPECTED_FILES:
        content = (b"weights" * 200_000 if name == "model.bin" else
                   b'["token"]' if name == "vocabulary.json" else b'{"ok":true}')
        (source / name).write_bytes(content)
    expected = {
        name: (path.stat().st_size, hashlib.sha256(path.read_bytes()).hexdigest())
        for name in installer.EXPECTED_FILES
        for path in [source / name]
    }
    monkeypatch.setattr(installer, "EXPECTED_FILES", expected)
    return source, tmp_path / "managed"


def destination(root: Path) -> Path:
    return root / "standard" / installer.REVISION


def assert_unpublished(root: Path) -> None:
    assert not destination(root).exists()
    assert not list((root / "standard").glob(".install-*"))


def wait_signal(process: subprocess.Popen, expected: bytes) -> None:
    assert process.stdout is not None
    lines = queue.Queue()
    threading.Thread(target=lambda: lines.put(process.stdout.readline()), daemon=True).start()
    try:
        line = lines.get(timeout=10)
    except queue.Empty:
        pytest.fail("probe child did not signal readiness within ten seconds")
    assert line.strip() == expected


def test_offline_import_verifies_copy_preserves_source_and_is_idempotent(model, monkeypatch):
    source, root = model
    original = {path.name: path.read_bytes() for path in source.iterdir()}
    monkeypatch.setitem(sys.modules, "huggingface_hub", None)  # No network library allowed.
    events = []
    installed = installer.import_standard(source, root, progress=events.append)
    assert installed == destination(root)
    assert {path.name: path.read_bytes() for path in source.iterdir()} == original
    assert installer.verify_model_dir(installed)
    manifest = json.loads((installed / "install-manifest.json").read_text())
    assert manifest["revision"] == installer.REVISION
    assert manifest["files"] == installer.verify_model_dir(installed)
    for phase in ("copying", "verifying"):
        counts = [event.completed_bytes for event in events if event.phase == phase]
        assert counts == sorted(counts)
        assert counts[-1] == sum(size for size, _ in installer.EXPECTED_FILES.values())
    assert events[-1].phase == "installed"
    before = (installed / "model.bin").stat().st_mtime_ns
    # A valid install is verified in-place, even if the old copy source disappeared.
    assert installer.import_standard(source / "absent", root) == installed
    assert (installed / "model.bin").stat().st_mtime_ns == before


@pytest.mark.parametrize("phase", ["copying", "verifying"])
def test_cancel_during_copy_or_hash_removes_staging_and_allows_retry(model, phase):
    source, root = model
    cancel = False
    events = []

    def progress(event):
        nonlocal cancel
        events.append(event)
        if event.phase == phase and event.completed_bytes > 0:
            cancel = True

    with pytest.raises(installer.InstallCancelled):
        installer.import_standard(source, root, progress=progress, cancelled=lambda: cancel)
    assert_unpublished(root)
    assert not any(event.phase == "installed" for event in events)
    assert (source / "model.bin").exists()
    assert installer.import_standard(source, root) == destination(root)


def test_cancel_before_start_does_not_create_root(model):
    source, root = model
    with pytest.raises(installer.InstallCancelled):
        installer.import_standard(source, root, cancelled=lambda: True)
    assert not root.exists()


def test_corrupt_copy_never_published_and_existing_corruption_is_preserved(model):
    source, root = model
    path = source / "model.bin"
    correct = path.read_bytes()
    path.write_bytes(b"x" * len(correct))
    with pytest.raises(ValueError, match="pinned release"):
        installer.import_standard(source, root)
    assert_unpublished(root)
    path.write_bytes(correct)
    installed = installer.import_standard(source, root)
    existing = installed / "model.bin"
    existing.write_bytes(b"y" * len(correct))
    with pytest.raises(ValueError, match="pinned release"):
        installer.import_standard(source, root)
    assert existing.read_bytes() == b"y" * len(correct)
    assert path.read_bytes() == correct


def test_disk_full_preflight_and_write_failure_allow_retry(model, monkeypatch):
    source, root = model
    original_usage = shutil.disk_usage
    monkeypatch.setattr(installer.shutil, "disk_usage",
                        lambda _: types.SimpleNamespace(free=0))
    with pytest.raises(OSError, match="insufficient disk"):
        installer.import_standard(source, root)
    assert_unpublished(root)
    monkeypatch.setattr(installer.shutil, "disk_usage", original_usage)
    original_fsync = os.fsync
    monkeypatch.setattr(installer.os, "fsync",
                        lambda _: (_ for _ in ()).throw(OSError("synthetic disk full")))
    with pytest.raises(OSError, match="synthetic disk full"):
        installer.import_standard(source, root)
    assert_unpublished(root)
    monkeypatch.setattr(installer.os, "fsync", original_fsync)
    assert installer.import_standard(source, root) == destination(root)


def test_destination_race_is_not_overwritten(model, monkeypatch):
    source, root = model
    original_rename = Path.rename

    def race(staging, target):
        target.mkdir()
        (target / "keep.txt").write_text("other process")
        return original_rename(staging, target)

    monkeypatch.setattr(Path, "rename", race)
    with pytest.raises(FileExistsError):
        installer.import_standard(source, root)
    assert (destination(root) / "keep.txt").read_text() == "other process"
    assert not list((root / "standard").glob(".install-*"))


def test_download_failure_cleans_only_current_staging_and_retry_uses_pinned_request(model, monkeypatch):
    source, root = model
    parent = root / "standard"
    parent.mkdir(parents=True)
    orphan = parent / ".install-old-unowned"
    orphan.mkdir()
    (orphan / "keep.txt").write_text("not ours")
    requests = []

    def download(**kwargs):
        requests.append(kwargs)
        (kwargs["local_dir"] / "partial").write_bytes(b"incomplete")
        raise RuntimeError("synthetic network interruption")

    hub = types.ModuleType("huggingface_hub")
    hub.snapshot_download = download
    monkeypatch.setitem(sys.modules, "huggingface_hub", hub)
    with pytest.raises(RuntimeError, match="network interruption"):
        installer.install_standard(root)
    assert not destination(root).exists()
    assert list(parent.glob(".install-*")) == [orphan]

    def completed_download(**kwargs):
        requests.append(kwargs)
        for path in source.iterdir():
            shutil.copyfile(path, kwargs["local_dir"] / path.name)
        cache = kwargs["local_dir"] / ".cache"
        cache.mkdir()
        (cache / "metadata").write_text("download metadata")

    hub.snapshot_download = completed_download
    installed = installer.install_standard(root)
    assert not (installed / ".cache").exists()
    assert all(request["repo_id"] == installer.REPO_ID and
               request["revision"] == installer.REVISION for request in requests)
    assert all(set(request["allow_patterns"]) == {*installer.EXPECTED_FILES, "README.md"}
               for request in requests)
    assert (orphan / "keep.txt").read_text() == "not ours"
    # A second install uses existing files and does not contact the downloader.
    assert installer.install_standard(root) == installed
    assert len(requests) == 2


def test_observer_errors_do_not_change_commit_result(model):
    source, root = model

    def broken_observer(_):
        raise RuntimeError("UI disconnected")

    assert installer.import_standard(source, root, progress=broken_observer) == destination(root)


def test_download_preflight_rejects_low_space_before_importing_network_library(model, monkeypatch):
    _, root = model
    monkeypatch.setitem(sys.modules, "huggingface_hub", None)
    monkeypatch.setattr(installer.shutil, "disk_usage",
                        lambda _: types.SimpleNamespace(free=0))
    with pytest.raises(OSError, match="insufficient disk"):
        installer.install_standard(root)
    assert_unpublished(root)


def test_another_process_blocks_writer_and_kill_releases_lock(model):
    source, root = model
    parent = root / "standard"
    parent.mkdir(parents=True)
    holder = subprocess.Popen(
        [sys.executable, "-u", "-c",
         "import sys; from pathlib import Path; "
         "from voiceweave_worker.model_install import _install_lock; "
         "lock = _install_lock(Path(sys.argv[1])); lock.__enter__(); "
         "print('locked', flush=True); sys.stdin.read()", str(parent)],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    )
    assert holder.stdout is not None
    try:
        wait_signal(holder, b"locked")
        # The process already owns the lock; reject without creating a staging copy.
        with pytest.raises(installer.InstallBusy):
            installer.import_standard(source, root)
        assert_unpublished(root)
    finally:
        holder.kill()
        holder.wait(timeout=10)
        for stream in (holder.stdin, holder.stdout, holder.stderr):
            if stream is not None:
                stream.close()
    assert installer.import_standard(source, root) == destination(root)


def test_cancel_after_final_hash_still_prevents_publication(model):
    source, root = model
    cancel = False

    def progress(event):
        nonlocal cancel
        if (event.phase == "verifying" and event.total_bytes is not None
                and event.completed_bytes == event.total_bytes):
            cancel = True

    with pytest.raises(installer.InstallCancelled):
        installer.import_standard(source, root, progress=progress, cancelled=lambda: cancel)
    assert_unpublished(root)


def test_manifest_write_failure_does_not_publish_and_allows_retry(model, monkeypatch):
    source, root = model
    original_write = Path.write_text

    def fail_manifest(path, *args, **kwargs):
        if path.name == "install-manifest.json":
            raise OSError("synthetic manifest write failure")
        return original_write(path, *args, **kwargs)

    monkeypatch.setattr(Path, "write_text", fail_manifest)
    with pytest.raises(OSError, match="manifest write failure"):
        installer.import_standard(source, root)
    assert_unpublished(root)
    monkeypatch.setattr(Path, "write_text", original_write)
    assert installer.import_standard(source, root) == destination(root)


def test_killed_install_leaves_no_published_model_and_next_process_can_retry(model):
    source, root = model
    process = subprocess.Popen(
        [sys.executable, "-u", "-c", """
import sys
from pathlib import Path
from voiceweave_worker.model_install import _install
def populate(staging):
    (staging / "partial").write_bytes(b"partial")
    print("staged", flush=True)
    sys.stdin.read()
_install(Path(sys.argv[1]), populate, expected_files={"model.bin": (7, "unused")})
""", str(root)],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    )
    assert process.stdout is not None
    try:
        wait_signal(process, b"staged")
    finally:
        process.kill()
        process.wait(timeout=10)
        for stream in (process.stdin, process.stdout, process.stderr):
            if stream is not None:
                stream.close()
    assert not destination(root).exists()
    # A crash leaves an unpublished orphan. Do not delete unowned directories by age.
    orphans = list((root / "standard").glob(".install-*"))
    assert len(orphans) == 1
    assert (orphans[0] / "partial").read_bytes() == b"partial"
    assert installer.import_standard(source, root) == destination(root)
    assert (orphans[0] / "partial").read_bytes() == b"partial"


def test_windows_junction_in_source_is_rejected_without_touching_target(model, tmp_path):
    source, root = model
    junction = tmp_path / "linked-source"
    # mklink is only used to create a test junction, never for deleting/moving paths.
    result = subprocess.run(["cmd", "/c", "mklink", "/J", str(junction), str(source)],
                            capture_output=True)
    assert result.returncode == 0, result.stderr
    try:
        with pytest.raises(ValueError, match="reparse"):
            installer.import_standard(junction, root)
        assert_unpublished(root)
        assert (source / "model.bin").exists()
    finally:
        junction.rmdir()  # Remove just the verified junction, not its contents.


def _run_json_cli(monkeypatch, capsys, source: Path, root: Path, *, cancel: bool):
    from voiceweave_worker import cuda_install
    gate = threading.Event()
    monkeypatch.setattr(cuda_install, "_wait_for_cancel_input",
                        (lambda: None) if cancel else (lambda: gate.wait(5)))
    monkeypatch.setattr("sys.argv", ["model_install", "--model-root", str(root),
                                     "--source", str(source), "--json-progress"])
    try:
        code = installer.main()
    finally:
        gate.set()
    lines = [json.loads(line) for line in capsys.readouterr().out.splitlines()]
    return code, lines


def test_json_import_reports_phases_and_installed_without_paths(model, monkeypatch, capsys) -> None:
    source, root = model
    code, lines = _run_json_cli(monkeypatch, capsys, source, root, cancel=False)
    assert code == 0
    assert lines[-1]["phase"] == "installed"
    assert {"copying", "verifying"} <= {line["phase"] for line in lines}
    assert str(source) not in json.dumps(lines) and str(root) not in json.dumps(lines)
    assert destination(root).is_dir()


def test_json_import_cancel_publishes_nothing(model, monkeypatch, capsys) -> None:
    source, root = model
    code, lines = _run_json_cli(monkeypatch, capsys, source, root, cancel=True)
    assert code == 3 and lines[-1] == {"phase": "cancelled"}
    assert_unpublished(root)


def test_json_import_failure_is_a_fixed_code(model, monkeypatch, capsys) -> None:
    source, root = model
    (source / "model.bin").write_bytes(b"damaged")
    code, lines = _run_json_cli(monkeypatch, capsys, source, root, cancel=False)
    assert code == 1 and lines[-1] == {"phase": "failed", "code": "install_failed"}
    assert_unpublished(root)
