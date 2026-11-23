"""List third-party licenses of what the app ships, offline, into docs/THIRD_PARTY_LICENSES.md.

Rust: crates reachable from the app through normal (not build/dev) dependencies on
Windows. Python: the worker's runtime requirement closure (faster-whisper and the
installers' huggingface_hub), excluding build/test tools. Items the user downloads
separately (model, CUDA runtime) are listed by hand below. Licenses that are not
plainly permissive are flagged for review; this is an inventory, not legal advice.
"""
from __future__ import annotations

import importlib.metadata as metadata
import json
import re
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PERMISSIVE = re.compile(r"^(MIT|Apache-2\.0|BSD-[23]-Clause|ISC|Zlib|Unicode-3\.0|Unicode-DFS-2016|"
                        r"0BSD|CC0-1\.0|BSL-1\.0|Unlicense|MIT-0|PSF-2\.0)$")
PYTHON_ROOTS = ["faster-whisper", "huggingface_hub", "numpy"]


def needs_review(expression: str) -> bool:
    # "A OR B": one permissive choice is enough. "A AND B": every part must be permissive.
    for alternative in re.split(r"\s+OR\s+|/", expression.replace("(", "").replace(")", "")):
        parts = [part.strip() for part in re.split(r"\s+AND\s+", alternative) if part.strip()]
        if parts and all(PERMISSIVE.match(part) for part in parts):
            return False
    return True


def rust_packages() -> list[tuple[str, str, str]]:
    output = subprocess.run(
        ["cargo", "metadata", "--offline", "--format-version", "1",
         "--filter-platform", "x86_64-pc-windows-msvc",
         "--manifest-path", str(ROOT / "src-tauri" / "Cargo.toml")],
        capture_output=True, check=True).stdout
    data = json.loads(output)
    packages = {package["id"]: package for package in data["packages"]}
    nodes = {node["id"]: node for node in data["resolve"]["nodes"]}
    root = data["resolve"]["root"]
    seen, stack = set(), [root]
    while stack:
        current = stack.pop()
        if current in seen:
            continue
        seen.add(current)
        for dep in nodes[current]["deps"]:
            # kind None = normal dependency; build and dev dependencies are not shipped.
            if any(kind["kind"] is None for kind in dep["dep_kinds"]):
                stack.append(dep["pkg"])
    seen.discard(root)
    return sorted((packages[i]["name"], packages[i]["version"],
                   packages[i].get("license") or "不明（要確認）") for i in seen)


def python_packages() -> list[tuple[str, str, str]]:
    def canonical(name: str) -> str:
        return re.sub(r"[-_.]+", "-", name).lower()

    found: dict[str, metadata.Distribution] = {}
    stack = list(PYTHON_ROOTS)
    while stack:
        name = canonical(stack.pop())
        if name in found:
            continue
        try:
            dist = metadata.distribution(name)
        except metadata.PackageNotFoundError:
            continue
        found[name] = dist
        for requirement in dist.requires or []:
            if "extra ==" in requirement:
                continue
            marker = requirement.split(";", 1)
            if len(marker) == 2 and ("sys_platform" in marker[1] and "win32" not in marker[1]
                                     and "!=" not in marker[1]):
                continue
            stack.append(re.split(r"[\s<>=!~\[;(]", requirement.strip(), maxsplit=1)[0])
    rows = []
    for dist in found.values():
        meta = dist.metadata
        license_text = meta.get("License-Expression") or ""
        if not license_text:
            classifiers = [c.split("::")[-1].strip() for c in meta.get_all("Classifier") or []
                           if c.startswith("License ::")]
            license_text = "; ".join(classifiers) or (meta.get("License") or "").splitlines()[0:1]
            license_text = license_text if isinstance(license_text, str) else "".join(license_text)
        rows.append((meta["Name"], dist.version, license_text.strip() or "不明（要確認）"))
    return sorted(rows, key=lambda row: row[0].lower())


def python_needs_review(text: str) -> bool:
    # Classifier names such as "MIT License" or "BSD License" are permissive too.
    if re.search(r"\b(MIT|BSD|Apache|ISC|PSF|Python Software Foundation)\b", text) and not re.search(
            r"GPL|LGPL|MPL|Mozilla|Proprietary|不明", text):
        return False
    return needs_review(text)


def table(rows, flag) -> list[str]:
    lines = ["| パッケージ | 版 | ライセンス | 要確認 |", "| --- | --- | --- | --- |"]
    lines += [f"| {name} | {version} | {license_text} | {'要確認' if flag(license_text) else ''} |"
              for name, version, license_text in rows]
    return lines


def main() -> int:
    rust = rust_packages()
    python = python_packages()
    by_license = defaultdict(int)
    for _, _, license_text in rust:
        by_license[license_text] += 1
    review_rust = [row for row in rust if needs_review(row[2])]
    review_python = [row for row in python if python_needs_review(row[2])]
    lines = [
        "# 第三者ライセンス一覧（自動生成）",
        "",
        "`python scripts/third-party-licenses.py` で再生成する。配布物に含まれるものの棚卸しで、法的判断ではない。"
        "「要確認」は寛容な許諾（MIT、Apache-2.0、BSD等）だけで説明できないもので、配布前に条件を確認する。",
        "",
        "## 利用者が別に取得するもの（同梱しない）",
        "",
        "| 項目 | 取得元 | ライセンス | 備考 |",
        "| --- | --- | --- | --- |",
        "| 標準モデル（faster-whisper-large-v3-turbo） | Hugging Face（固定revision） | MIT | 設定画面から導入 |",
        "| NVIDIA cuBLAS（CUDA 12） | PyPI `nvidia-cublas-cu12`（固定版） | NVIDIA独自（`LicenseRef-NVIDIA-Proprietary`） | GPU高速化の導入時に取得。要確認 |",
        "",
        "## 同梱物に含まれるもの（補足）",
        "",
        "- Silero VAD（`faster_whisper/assets/silero_vad_v6.onnx`）: faster-whisperに同梱、MIT。",
        "- PyInstallerのブートローダー: GPL-2.0（ブートローダー例外付き。生成物の配布は例外の対象）。要確認。",
        "- PyAV（`av`）のwheelはFFmpegのライブラリを同梱する。PyAV自体はBSD-3-Clauseだが、FFmpeg部分はLGPL系。配布条件（動的リンクの維持、ソース入手先の明示等）を要確認。",
        "",
        f"## Rust（通常の依存 {len(rust)}件、うち要確認 {len(review_rust)}件）",
        "",
        "ライセンス別の件数: " + "、".join(f"{k} {v}件" for k, v in sorted(by_license.items(), key=lambda kv: -kv[1])),
        "",
        "### 要確認",
        "",
        *(table(review_rust, needs_review) if review_rust else ["なし"]),
        "",
        f"## Python（ワーカーの実行に必要なもの {len(python)}件、うち要確認 {len(review_python)}件）",
        "",
        *table(python, python_needs_review),
        "",
        "## Rust（全件）",
        "",
        *table(rust, needs_review),
        "",
    ]
    out = ROOT / "docs" / "THIRD_PARTY_LICENSES.md"
    out.write_text("\n".join(lines), encoding="utf-8", newline="\n")
    print(f"rust={len(rust)} (review {len(review_rust)}), python={len(python)} (review {len(review_python)}) -> {out}")
    for name, version, license_text in review_rust + review_python:
        print(f"  review: {name} {version}: {license_text}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
