"""Build a local wheel offline and check that the shared catalog remains importable."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import zipfile


def main() -> int:
    project = Path(__file__).resolve().parent.parent
    with tempfile.TemporaryDirectory(prefix="voiceweave-catalog-wheel-") as temporary:
        result = subprocess.run(
            [sys.executable, "-m", "pip", "wheel", str(project / "worker"),
             "--wheel-dir", temporary, "--no-index", "--no-deps", "--no-build-isolation",
             "--disable-pip-version-check"],
            capture_output=True, timeout=120,
        )
        if result.returncode != 0:
            raise RuntimeError("offline worker wheel build failed")
        wheels = list(Path(temporary).glob("*.whl"))
        if len(wheels) != 1:
            raise RuntimeError("expected one worker wheel")
        wheel = wheels[0]
        original = (project / "worker/voiceweave_worker/model_catalog.json").read_bytes()
        with zipfile.ZipFile(wheel) as archive:
            if archive.read("voiceweave_worker/model_catalog.json") != original:
                raise RuntimeError("packaged catalog differs from shared source")
        # Import the wheel itself, not the editable package already installed in .venv.
        script = """
import json, sys
sys.path.insert(0, sys.argv[1])
from voiceweave_worker import model_install
assert model_install.__file__.startswith(sys.argv[1])
print(json.dumps({"revision": model_install.REVISION, "files": len(model_install.EXPECTED_FILES)}))
"""
        probe = subprocess.run([sys.executable, "-I", "-c", script, str(wheel)],
                               capture_output=True, timeout=20)
        if probe.returncode != 0:
            raise RuntimeError("catalog import from packaged wheel failed")
        metadata = json.loads(probe.stdout)
        if metadata["revision"] != json.loads(original)["revision"] or metadata["files"] != 5:
            raise RuntimeError("packaged metadata does not match source")
    print(json.dumps({"ok": True, "catalog_identical": True, "zip_import": True,
                      "files": metadata["files"]}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
