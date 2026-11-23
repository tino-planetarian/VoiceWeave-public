import hashlib
import json
from pathlib import Path

import pytest

from voiceweave_worker.model_install import (
    CATALOG, EXPECTED_FILES, OPTIONAL_CATALOGS, PRESETS, _load_catalog, verify_model_dir,
)


def test_verification_requires_complete_unchanged_weights(tmp_path: Path) -> None:
    for name in EXPECTED_FILES:
        if name == "model.bin":
            content = b"weights"
        elif name == "vocabulary.json":
            content = json.dumps(["token"]).encode()
        else:
            content = json.dumps({"ok": True}).encode()
        (tmp_path / name).write_bytes(content)

    expected = {
        name: ((tmp_path / name).stat().st_size, hashlib.sha256((tmp_path / name).read_bytes()).hexdigest())
        for name in EXPECTED_FILES
    }
    files = verify_model_dir(tmp_path, expected_files=expected)
    assert set(files) == set(EXPECTED_FILES)

    (tmp_path / "model.bin").write_bytes(b"changed")
    with pytest.raises(ValueError, match="pinned release"):
        verify_model_dir(tmp_path, expected_files=expected)

    (tmp_path / "model.bin").write_bytes(b"weights")
    (tmp_path / "tokenizer.json").unlink()
    with pytest.raises(ValueError, match="tokenizer.json"):
        verify_model_dir(tmp_path, expected_files=expected)


@pytest.mark.parametrize("mutation", ["schema", "schema_bool", "revision", "hash", "path", "duplicate", "zero"])
def test_packaged_catalog_rejects_unsupported_or_unsafe_metadata(tmp_path: Path, mutation: str) -> None:
    catalog = json.loads(json.dumps(CATALOG))
    if mutation == "schema":
        catalog["schema_version"] = 2
    elif mutation == "schema_bool":
        catalog["schema_version"] = True
    elif mutation == "revision":
        catalog["revision"] = "main"
    elif mutation == "hash":
        catalog["files"][0]["sha256"] = "unverified"
    elif mutation == "path":
        catalog["files"][0]["name"] = "../config.json"
    elif mutation == "duplicate":
        catalog["files"][1]["name"] = catalog["files"][0]["name"]
    else:
        catalog["files"][0]["bytes"] = 0
    path = tmp_path / "catalog.json"
    path.write_text(json.dumps(catalog), encoding="utf-8")
    with pytest.raises(ValueError):
        _load_catalog(path)

def test_parakeet_catalogs_are_pinned_and_installable():
    assert {"parakeet-ja", "parakeet-en"} <= set(PRESETS)
    ja = OPTIONAL_CATALOGS["parakeet-ja"]
    assert ja["license"] == "cc-by-4.0"
    assert {file["name"] for file in ja["files"]} == {"model.int8.onnx", "tokens.txt"}
    en = OPTIONAL_CATALOGS["parakeet-en"]
    assert {file["name"] for file in en["files"]} == {
        "encoder.int8.onnx", "decoder.int8.onnx", "joiner.int8.onnx", "tokens.txt"}


def test_a_parakeet_catalog_with_another_license_is_refused(tmp_path):
    import json

    from voiceweave_worker.model_install import files as package_files

    catalog = json.loads(package_files("voiceweave_worker").joinpath(
        "model_catalog_parakeet_ja.json").read_text(encoding="utf-8"))
    catalog["license"] = "mit"
    path = tmp_path / "catalog.json"
    path.write_text(json.dumps(catalog), encoding="utf-8")
    with pytest.raises(ValueError):
        _load_catalog(path, "parakeet-ja")
