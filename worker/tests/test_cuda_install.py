import hashlib
import json
import zipfile
from pathlib import Path

import pytest

from voiceweave_worker import cuda_install
from voiceweave_worker.cuda_install import install_cuda, verify_cuda_dir
from voiceweave_worker.model_install import InstallBusy, InstallCancelled, _install_lock

BLAS = b"synthetic-cublas" * 8
BLAS_LT = b"synthetic-cublaslt" * 8


def _sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _wheel(path: Path, *, blas: bytes = BLAS, extra: dict[str, bytes] | None = None) -> dict:
    members = {
        "nvidia/cublas/bin/cublas64_12.dll": blas,
        "nvidia/cublas/bin/cublasLt64_12.dll": BLAS_LT,
        "nvidia/cublas/bin/nvblas64_12.dll": b"unwanted",
        **(extra or {}),
    }
    with zipfile.ZipFile(path, "w") as archive:
        for name, data in members.items():
            archive.writestr(name, data)
    return {
        **cuda_install.CATALOG,
        "wheel": {**cuda_install.CATALOG["wheel"], "bytes": path.stat().st_size,
                  "sha256": _sha(path.read_bytes())},
        "members": [
            {"wheel_path": "nvidia/cublas/bin/cublas64_12.dll", "name": "cublas64_12.dll",
             "bytes": len(BLAS), "sha256": _sha(BLAS)},
            {"wheel_path": "nvidia/cublas/bin/cublasLt64_12.dll", "name": "cublasLt64_12.dll",
             "bytes": len(BLAS_LT), "sha256": _sha(BLAS_LT)},
        ],
    }


def test_packaged_catalog_is_pinned_and_bounded() -> None:
    catalog = cuda_install.CATALOG
    assert catalog["wheel"]["url"].startswith("https://files.pythonhosted.org/")
    assert {m["name"] for m in catalog["members"]} == {"cublas64_12.dll", "cublasLt64_12.dll"}


def test_installs_only_catalogued_members_and_records_manifest(tmp_path: Path) -> None:
    catalog = _wheel(tmp_path / "pinned.whl")
    phases: list[str] = []
    root = tmp_path / "data"
    installed = install_cuda(root, wheel=tmp_path / "pinned.whl", catalog=catalog,
                             progress=lambda p: phases.append(p.phase))
    assert installed == root.resolve() / "cuda"
    assert sorted(p.name for p in installed.iterdir()) == [
        "cublas64_12.dll", "cublasLt64_12.dll", "install-manifest.json"]
    manifest = json.loads((installed / "install-manifest.json").read_text())
    assert manifest["files"]["cublas64_12.dll"] == _sha(BLAS)
    assert phases[-1] == "installed" and "extracting" in phases
    assert [p.name for p in root.iterdir() if p.name.startswith(".install-")] == []
    verify_cuda_dir(installed, catalog=catalog)


def test_wheel_with_wrong_hash_is_rejected_before_extraction(tmp_path: Path) -> None:
    catalog = _wheel(tmp_path / "pinned.whl")
    catalog["wheel"]["sha256"] = "0" * 64
    root = tmp_path / "data"
    with pytest.raises(ValueError):
        install_cuda(root, wheel=tmp_path / "pinned.whl", catalog=catalog)
    assert not (root / "cuda").exists()
    assert [p.name for p in root.iterdir() if p.name.startswith(".install-")] == []


def test_tampered_member_never_publishes(tmp_path: Path) -> None:
    catalog = _wheel(tmp_path / "pinned.whl", blas=b"X" * len(BLAS))
    # The wheel hash was computed from the tampered file, so only the member check can catch it.
    root = tmp_path / "data"
    with pytest.raises(ValueError):
        install_cuda(root, wheel=tmp_path / "pinned.whl", catalog=catalog)
    assert not (root / "cuda").exists()
    assert [p.name for p in root.iterdir() if p.name.startswith(".install-")] == []


def test_existing_install_is_verified_not_overwritten_or_deleted(tmp_path: Path) -> None:
    catalog = _wheel(tmp_path / "pinned.whl")
    root = tmp_path / "data"
    installed = install_cuda(root, wheel=tmp_path / "pinned.whl", catalog=catalog)
    assert install_cuda(root, wheel=tmp_path / "pinned.whl", catalog=catalog) == installed
    (installed / "cublas64_12.dll").write_bytes(b"damaged")
    with pytest.raises(ValueError):
        install_cuda(root, wheel=tmp_path / "pinned.whl", catalog=catalog)
    assert (installed / "cublas64_12.dll").read_bytes() == b"damaged"


def test_cancellation_leaves_nothing_published(tmp_path: Path) -> None:
    catalog = _wheel(tmp_path / "pinned.whl")
    root = tmp_path / "data"
    with pytest.raises(InstallCancelled):
        install_cuda(root, wheel=tmp_path / "pinned.whl", catalog=catalog, cancelled=lambda: True)
    assert not (root / "cuda").exists()


def test_concurrent_installer_is_refused(tmp_path: Path) -> None:
    catalog = _wheel(tmp_path / "pinned.whl")
    root = tmp_path / "data"
    root.mkdir()
    with _install_lock(root.resolve()):
        with pytest.raises(InstallBusy):
            install_cuda(root, wheel=tmp_path / "pinned.whl", catalog=catalog)


def test_download_url_must_stay_on_the_pinned_host(tmp_path: Path) -> None:
    bad = json.loads(Path(cuda_install.__file__).with_name("cuda_catalog.json").read_text())
    bad["wheel"]["url"] = "https://example.invalid/x.whl"
    path = tmp_path / "bad.json"
    path.write_text(json.dumps(bad))
    with pytest.raises(ValueError):
        cuda_install._load_catalog(path)


class _FakeResponse:
    def __init__(self, data: bytes, url: str = "https://files.pythonhosted.org/x.whl") -> None:
        self._data, self._url = data, url

    def __enter__(self):
        return self

    def __exit__(self, *exc) -> None:
        return None

    def geturl(self) -> str:
        return self._url

    def read(self, size: int) -> bytes:
        chunk, self._data = self._data[:size], self._data[size:]
        return chunk


def test_download_streams_into_staging_and_installs(tmp_path: Path, monkeypatch) -> None:
    source = tmp_path / "pinned.whl"
    catalog = _wheel(source)
    monkeypatch.setattr(cuda_install.urllib.request, "urlopen",
                        lambda request, timeout: _FakeResponse(source.read_bytes()))
    installed = install_cuda(tmp_path / "data", catalog=catalog)
    verify_cuda_dir(installed, catalog=catalog)


@pytest.mark.parametrize("response", [
    _FakeResponse(b"x" * 10_000_000),
    _FakeResponse(b"x", url="https://evil.example/x.whl"),
])
def test_download_rejects_oversize_and_foreign_redirect(tmp_path: Path, monkeypatch, response) -> None:
    catalog = _wheel(tmp_path / "pinned.whl")
    monkeypatch.setattr(cuda_install.urllib.request, "urlopen", lambda request, timeout: response)
    root = tmp_path / "data"
    with pytest.raises(ValueError):
        install_cuda(root, catalog=catalog)
    assert not (root / "cuda").exists()
    assert [p.name for p in root.iterdir() if p.name.startswith(".install-")] == []


def _run_cli(monkeypatch, capsys, tmp_path: Path, catalog: dict, *, cancel: bool) -> tuple[int, list[dict]]:
    import threading
    gate = threading.Event()
    # Stands in for the parent: either it cancels at once, or it keeps stdin open.
    monkeypatch.setattr(cuda_install, "_wait_for_cancel_input",
                        (lambda: None) if cancel else (lambda: gate.wait(5)))
    monkeypatch.setattr(cuda_install, "CATALOG", catalog)
    monkeypatch.setattr("sys.argv", ["cuda_install", "--root", str(tmp_path / "data"),
                                     "--wheel", str(tmp_path / "pinned.whl"), "--json-progress"])
    try:
        code = cuda_install.main()
    finally:
        gate.set()
    lines = [json.loads(line) for line in capsys.readouterr().out.splitlines()]
    return code, lines


def test_json_progress_reports_phases_without_paths(tmp_path: Path, monkeypatch, capsys) -> None:
    catalog = _wheel(tmp_path / "pinned.whl")
    code, lines = _run_cli(monkeypatch, capsys, tmp_path, catalog, cancel=False)
    assert code == 0
    assert lines[-1] == {"phase": "installed", "completed": 0, "total": 0}
    assert {"checking-wheel", "extracting", "verifying"} <= {line["phase"] for line in lines}
    assert str(tmp_path) not in json.dumps(lines)


def test_stdin_input_or_eof_cancels_without_publishing(tmp_path: Path, monkeypatch, capsys) -> None:
    catalog = _wheel(tmp_path / "pinned.whl")
    code, lines = _run_cli(monkeypatch, capsys, tmp_path, catalog, cancel=True)
    assert code == 3 and lines[-1] == {"phase": "cancelled"}
    assert not (tmp_path / "data" / "cuda").exists()


def test_failure_output_is_a_fixed_code(tmp_path: Path, monkeypatch, capsys) -> None:
    catalog = _wheel(tmp_path / "pinned.whl")
    catalog["wheel"]["sha256"] = "0" * 64
    code, lines = _run_cli(monkeypatch, capsys, tmp_path, catalog, cancel=False)
    assert code == 1 and lines[-1] == {"phase": "failed", "code": "install_failed"}
