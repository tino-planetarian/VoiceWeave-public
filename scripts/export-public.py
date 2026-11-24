"""Copy what goes into the public repository (VoiceWeave-public) into a folder.

Only files tracked by Git are copied, minus the development records, and the files in
public/ replace their counterparts (README, AGENTS.md, CLAUDE.md, LICENSE). The user manual
(manual/) goes to docs/, which GitHub Pages serves.

    python scripts/export-public.py <destination folder>

The destination's own .git folder is kept, so the public repository can be updated by
running this again and committing there. Everything else in it is replaced.
"""
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
# Development records and drafts: not published.
EXCLUDED_FILES = {"HANDOFF.md", "TODO.md", "AGENTS.md", "CLAUDE.md", "README.md"}
EXCLUDED_DIRS = ("public/", "docs/")
KEPT_DOCS = {"docs/THIRD_PARTY_LICENSES.md"}


def published(path: str) -> bool:
    if path in KEPT_DOCS:
        return True
    if path in EXCLUDED_FILES:
        return False
    return not path.startswith(EXCLUDED_DIRS)


def main() -> None:
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    destination = Path(sys.argv[1]).resolve()
    if destination == ROOT or ROOT in destination.parents:
        sys.exit("The destination must be outside this repository.")
    destination.mkdir(parents=True, exist_ok=True)
    for child in destination.iterdir():
        if child.name == ".git":
            continue
        shutil.rmtree(child) if child.is_dir() else child.unlink()
    tracked = subprocess.run(
        ["git", "ls-files", "-z"], cwd=ROOT, capture_output=True, check=True
    ).stdout.decode("utf-8").split("\0")
    count = 0
    for path in filter(None, tracked):
        if not published(path):
            continue
        target = destination / ("docs/" + path[len("manual/"):] if path.startswith("manual/") else path)
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / path, target)
        count += 1
    for source in (ROOT / "public").iterdir():
        shutil.copy2(source, destination / source.name)
        count += 1
    print(f"{count} files copied to {destination}")


if __name__ == "__main__":
    main()
