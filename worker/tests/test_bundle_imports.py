"""The packaged worker (PyInstaller) only contains modules it can find from the entry
point's top-level imports; modules imported inside functions must be listed in the spec's
hiddenimports. 0.3.2 and 0.3.3 shipped without `resident` and `topics` and the packaged
worker could not prepare a model (2026-10-15). This test keeps the list complete."""
import ast
from pathlib import Path

WORKER = Path(__file__).resolve().parents[1]
PACKAGE = WORKER / "voiceweave_worker"


def lazily_imported_modules() -> set[str]:
    found = set()
    for path in PACKAGE.glob("*.py"):
        tree = ast.parse(path.read_text(encoding="utf-8"))
        top_level = set(map(id, tree.body))
        for node in ast.walk(tree):
            if not isinstance(node, ast.ImportFrom) or id(node) in top_level:
                continue
            if node.level == 1 and node.module:
                found.add(f"voiceweave_worker.{node.module}")
            elif node.module and node.module.startswith("voiceweave_worker."):
                found.add(node.module)
    return found


def test_every_lazily_imported_worker_module_is_bundled():
    spec = (WORKER / "voiceweave-worker.spec").read_text(encoding="utf-8")
    missing = sorted(name for name in lazily_imported_modules() if f"'{name}'" not in spec)
    assert missing == [], f"add to hiddenimports in voiceweave-worker.spec: {missing}"


def test_the_check_sees_the_known_lazy_imports():
    assert {"voiceweave_worker.resident", "voiceweave_worker.topics",
            "voiceweave_worker.streaming"} <= lazily_imported_modules()
