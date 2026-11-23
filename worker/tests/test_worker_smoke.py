import subprocess
import os
import sys
from pathlib import Path
import io
import pytest

from voiceweave_worker.protocol import read_frame, write_frame


def test_worker_reports_missing_local_model_and_recovers(tmp_path: Path) -> None:
    worker = subprocess.Popen(
        [sys.executable, "-m", "voiceweave_worker"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    assert worker.stdin is not None
    assert worker.stdout is not None
    try:
        hello = read_frame(worker.stdout)
        assert hello is not None and hello[0]["kind"] == "hello"
        write_frame(
            worker.stdin,
            {"kind": "prepare", "request_id": "missing", "model_dir": str(tmp_path / "missing")},
        )
        response = read_frame(worker.stdout)
        assert response is not None
        assert response[0]["kind"] == "error"
        assert response[0]["code"] == "MODEL_MISSING"
        write_frame(worker.stdin, {"kind": "shutdown", "request_id": "bye"})
        reply = read_frame(worker.stdout)
        assert reply is not None and reply[0]["kind"] == "bye"
    finally:
        worker.stdin.close()
        worker.wait(timeout=10)
    assert worker.returncode == 0


def test_worker_rejects_corrupt_protocol() -> None:
    worker = subprocess.Popen(
        [sys.executable, "-m", "voiceweave_worker"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    assert worker.stdin is not None
    assert worker.stdout is not None
    assert read_frame(worker.stdout) is not None
    worker.stdin.write((65_537).to_bytes(4, "little"))
    worker.stdin.close()
    assert worker.wait(timeout=10) == 2


def test_lazy_warmup_failure_never_announces_ready(tmp_path: Path) -> None:
    # Loading weights can succeed while native inference fails on iteration.
    # A synthetic backend exercises that boundary without real models or speech.
    (tmp_path / "faster_whisper.py").write_text(
        "class WhisperModel:\n"
        "    def __init__(self, *args, **kwargs): pass\n"
        "    def transcribe(self, audio, **kwargs):\n"
        "        assert kwargs['vad_filter'] is False\n"
        "        def segments():\n"
        "            yield object()\n"
        "            raise RuntimeError('synthetic native inference failure')\n"
        "        return segments(), None\n",
        encoding="utf-8",
    )
    model_dir = tmp_path / "model"
    model_dir.mkdir()
    (model_dir / "model.bin").write_bytes(b"synthetic")
    env = dict(os.environ)
    env["PYTHONPATH"] = str(tmp_path) + os.pathsep + str(Path(__file__).resolve().parents[1])
    worker = subprocess.Popen(
        [sys.executable, "-m", "voiceweave_worker"],
        cwd=tmp_path,
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    assert worker.stdin is not None and worker.stdout is not None
    try:
        assert read_frame(worker.stdout)[0]["kind"] == "hello"
        write_frame(worker.stdin, {"kind": "prepare", "request_id": "prepare", "model_dir": str(model_dir)})
        response = read_frame(worker.stdout)
        assert response[0]["kind"] == "error"
        assert response[0]["code"] == "WORKER_OPERATION_FAILED"
        write_frame(worker.stdin, {"kind": "transcribe", "request_id": "unprepared"})
        response = read_frame(worker.stdout)
        assert response[0]["code"] == "MODEL_NOT_READY"
        write_frame(worker.stdin, {"kind": "shutdown", "request_id": "bye"})
        assert read_frame(worker.stdout)[0]["kind"] == "bye"
    finally:
        worker.stdin.close()
        worker.wait(timeout=10)
    assert worker.returncode == 0


@pytest.mark.parametrize("fast_window", [False, True])
@pytest.mark.parametrize("fail_decode", [False, True])
def test_worker_reports_timings_only_for_completed_recognition(
        tmp_path: Path, fail_decode: bool, fast_window: bool) -> None:
    # Run the real worker protocol around a synthetic backend. No microphone,
    # downloaded model, personal text, or unbounded pipe reads are needed.
    (tmp_path / "faster_whisper.py").write_text(
        "from types import SimpleNamespace\n"
        "class WhisperModel:\n"
        "    def __init__(self, *args, **kwargs): pass\n"
        "    def transcribe(self, audio, **kwargs):\n"
        "        if not kwargs['vad_filter']: return iter(()), SimpleNamespace(language='ja')\n"
        "        def segments():\n"
        "            yield SimpleNamespace(text='synthetic private fragment')\n"
        f"            if {fail_decode!r}: raise RuntimeError('synthetic sensitive exception')\n"
        "        return segments(), SimpleNamespace(language='ja')\n",
        encoding="utf-8",
    )
    model_dir = tmp_path / "model"
    model_dir.mkdir()
    (model_dir / "model.bin").write_bytes(b"synthetic")
    env = dict(os.environ)
    env["PYTHONPATH"] = str(tmp_path) + os.pathsep + str(Path(__file__).resolve().parents[1])
    requests = io.BytesIO()
    write_frame(requests, {"kind": "prepare", "request_id": "prepare", "model_dir": str(model_dir)})
    write_frame(requests, {"kind": "transcribe", "request_id": "speech", "session_id": "synthetic-session",
        "sample_rate": 16000, "channels": 1, "sample_count": 16000, "fast_window": fast_window},
        bytes(64000))
    write_frame(requests, {"kind": "shutdown", "request_id": "bye"})
    result = subprocess.run([sys.executable, "-m", "voiceweave_worker"], cwd=tmp_path,
        env=env, input=requests.getvalue(), capture_output=True, timeout=30)
    assert result.returncode == 0 and result.stderr == b""
    replies = io.BytesIO(result.stdout)
    assert read_frame(replies)[0]["kind"] == "hello"
    assert read_frame(replies)[0]["kind"] == "ready"
    header, payload = read_frame(replies)
    assert payload == b""
    if fail_decode:
        assert header["kind"] == "error"
        assert header["code"] == "WORKER_OPERATION_FAILED"
        assert "raw_text" not in header and "elapsed_ms" not in header
        assert b"synthetic private fragment" not in result.stdout
        assert b"synthetic sensitive exception" not in result.stdout
    else:
        assert header["kind"] == "result"
        assert header["session_id"] == "synthetic-session"
        assert header["raw_text"] == "synthetic private fragment"
        assert type(header["elapsed_ms"]) is int and header["elapsed_ms"] >= 0
        if fast_window:
            # The synthetic backend has no fast path, so the full path decided and only
            # the honest total (including the abandoned attempt) is reported.
            assert header["recognition_path"] == "short_fallback"
            assert header["fallback_reason"] == "exception"
            assert "frontend_ms" not in header and "decode_ms" not in header
        else:
            assert header["recognition_path"] == "full"
            assert "fallback_reason" not in header
            for field in ("frontend_ms", "decode_ms"):
                assert type(header[field]) is int and header[field] >= 0
            assert abs(header["frontend_ms"] + header["decode_ms"] - header["elapsed_ms"]) <= 2
    assert read_frame(replies)[0]["kind"] == "bye"
    assert read_frame(replies) is None


def _prepare_replies(tmp_path: Path, header_extra: dict) -> list[dict]:
    # The synthetic backend records the device it was asked for in its ready-time warmup.
    (tmp_path / "faster_whisper.py").write_text(
        "from types import SimpleNamespace\n"
        "class WhisperModel:\n"
        "    def __init__(self, path, **kwargs):\n"
        "        assert kwargs['device'] in ('cpu', 'cuda')\n"
        "    def transcribe(self, audio, **kwargs): return iter(()), SimpleNamespace(language='ja')\n",
        encoding="utf-8",
    )
    model_dir = tmp_path / "model"
    model_dir.mkdir()
    (model_dir / "model.bin").write_bytes(b"synthetic")
    env = dict(os.environ)
    env["PYTHONPATH"] = str(tmp_path) + os.pathsep + str(Path(__file__).resolve().parents[1])
    requests = io.BytesIO()
    write_frame(requests, {"kind": "prepare", "request_id": "prepare", "model_dir": str(model_dir), **header_extra})
    write_frame(requests, {"kind": "shutdown", "request_id": "bye"})
    result = subprocess.run([sys.executable, "-m", "voiceweave_worker"], cwd=tmp_path,
        env=env, input=requests.getvalue(), capture_output=True, timeout=15)
    assert result.returncode == 0 and result.stderr == b""
    replies = io.BytesIO(result.stdout)
    assert read_frame(replies)[0]["kind"] == "hello"
    return [read_frame(replies)[0], read_frame(replies)[0]]


@pytest.mark.parametrize("extra", [
    {"device": "cuda"},
    {"device": "cuda", "cuda_dir": "relative"},
    {"device": "cuda", "cuda_dir": "{tmp}/empty"},
    {"device": "tpu"},
])
def test_cuda_prepare_requires_a_complete_absolute_library_directory(tmp_path: Path, extra: dict) -> None:
    (tmp_path / "empty").mkdir()
    extra = {k: v.replace("{tmp}", str(tmp_path)) if isinstance(v, str) else v for k, v in extra.items()}
    reply, _ = _prepare_replies(tmp_path, extra)
    assert reply["kind"] == "error" and reply["code"] == "WORKER_OPERATION_FAILED"


def test_cuda_prepare_reports_the_device_when_libraries_are_complete(tmp_path: Path) -> None:
    libraries = tmp_path / "cuda"
    libraries.mkdir()
    for name in ("cublas64_12.dll", "cublasLt64_12.dll"):
        (libraries / name).write_bytes(b"")
    reply, _ = _prepare_replies(tmp_path, {"device": "cuda", "compute_type": "int8_float16",
        "cuda_dir": str(libraries)})
    assert reply["kind"] == "ready" and reply["device"] == "cuda"


def test_repeat_detector_flags_loops_but_not_ordinary_speech() -> None:
    from voiceweave_worker.short_window import has_immediate_repeat
    loop = "エラー処理も忘れないでください" * 2
    assert has_immediate_repeat(loop)
    assert has_immediate_repeat("先ほどの説明で分からなかった。先ほどの説明で分からなかった。")
    for ordinary in ["まあまあですね", "はいはいはい分かりました", "明日の会議は三時からです。明日は雨です。", ""]:
        assert not has_immediate_repeat(ordinary)


def test_density_guard_flags_text_longer_than_the_speech() -> None:
    from voiceweave_worker.short_window import too_dense
    # The 2026-10-06 hallucination: one 3.8 s utterance written out almost twice.
    doubled = ("この文章を意味を変えずにもう少し短く読みやすく書き直してください"
               "この文章を意味を変えずにもう少し短く読みやすく書き直")
    assert too_dense(doubled, 3.8)
    # Very fast but real speech (11.7 characters per second) and short words pass.
    assert not too_dense("エラーメッセージを日本語にして利用者が次に何をすればいいかわかるようにしてください", 3.6)
    assert not too_dense("はい、分かりました。", 0.4)


def test_text_starting_after_the_speech_is_dropped() -> None:
    from voiceweave_worker.short_window import spoken_text_tokens
    ts = 50000  # first timestamp token; each step is 0.02 s
    at = lambda seconds: ts + round(seconds / 0.02)  # noqa: E731
    # Speech lasts 3.0 s. Segment 1 (0.0-2.8 s) is speech; segment 2 starts at 3.4 s,
    # inside the silent padding, where Whisper invents a closing line.
    tokens = [at(0.0), 1, 2, 3, at(2.8), at(3.4), 7, 8, at(5.0)]
    assert spoken_text_tokens(tokens, ts, 3.0) == [1, 2, 3]
    # A last segment without a closing timestamp is kept when it starts in the speech.
    assert spoken_text_tokens([at(0.0), 1, at(1.0), at(1.2), 4, 5], ts, 3.0) == [1, 4, 5]
    # No timestamps at all: everything is kept.
    assert spoken_text_tokens([1, 2, 3], ts, 3.0) == [1, 2, 3]


def test_fast_path_defaults_to_the_evaluated_fifteen_second_window() -> None:
    import inspect
    from voiceweave_worker.short_window import transcribe_short_window
    # 1500 frames (15 s) matched the full window's error rate on the evaluation set;
    # 1000 frames depended on the fallback guards. See docs/VALIDATION.md.
    assert inspect.signature(transcribe_short_window).parameters["min_frames"].default == 1500


@pytest.mark.parametrize("tool, marker", [("--cuda-install", "cuBLAS"), ("--model-install", "standard model")])
def test_bundled_installers_are_reachable_through_the_worker_entry_point(tool: str, marker: str) -> None:
    # The packaged worker is a single executable, so installers are dispatched by flag.
    result = subprocess.run([sys.executable, "-m", "voiceweave_worker", tool, "--help"],
                            capture_output=True, timeout=60, cwd=Path(__file__).resolve().parents[1])
    assert result.returncode == 0
    assert marker.encode() in result.stdout
    # No hello frame: the recognition protocol did not start.
    assert not result.stdout.startswith(b"\x00") and b'"kind"' not in result.stdout


def test_fast_path_finds_no_speech_in_silence_or_faint_noise() -> None:
    # Regression: without the voice-activity step the shortened window decoded silence
    # into invented sentences. The real (bundled) Silero detector is used; no speech.
    import numpy as np
    from voiceweave_worker.short_window import _speech_only
    rng = np.random.default_rng(7)
    assert _speech_only(np.zeros(48_000, dtype=np.float32)) is None
    assert _speech_only((rng.standard_normal(48_000) * 0.003).astype(np.float32)) is None
