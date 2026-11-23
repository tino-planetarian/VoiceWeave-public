"""Long-lived STT process. Importing this module never downloads a model."""

from __future__ import annotations

import os
import sys
import time
from pathlib import Path

from voiceweave_worker import __version__
from voiceweave_worker.protocol import ProtocolError, read_frame, write_frame
from voiceweave_worker.short_window import transcribe_short_window
from voiceweave_worker.timing import transcribe_measured


CUDA_LIBRARIES = ("cublas64_12.dll", "cublasLt64_12.dll")
# The style example for English recordings recognized by Whisper (the English shortcut).
ENGLISH_PROMPT = "Hello, welcome. This is a clear, well-punctuated English transcript."


def _enable_cuda_libraries(directory: object) -> None:
    """Make only the caller-named CUDA runtime directory loadable, if it is complete."""
    path = Path(directory) if isinstance(directory, str) else None
    if path is None or not path.is_absolute():
        raise ValueError("cuda_dir")
    if not all((path / name).is_file() for name in CUDA_LIBRARIES):
        raise OSError("cuda libraries missing")
    os.add_dll_directory(str(path))
    os.environ["PATH"] = str(path) + os.pathsep + os.environ.get("PATH", "")


def _keep_resident() -> int | None:
    """Ask Windows to keep the worker in memory; None if that is not possible."""
    try:
        from voiceweave_worker.resident import keep_resident

        return keep_resident()
    except Exception:
        return None


def _send(kind: str, request_id: str | None, **fields: object) -> None:
    write_frame(sys.stdout.buffer, {"kind": kind, "request_id": request_id, **fields})


_TOOLS = ("--cuda-install", "--model-install")


def _run_tool(name: str, arguments: list[str]) -> int:
    """Explicit, user-triggered installers bundled in the same executable.

    They run before the offline switch below because a download needs the network;
    the recognition loop itself never reaches this code.
    """
    sys.argv = [f"{sys.argv[0]} {name}", *arguments]
    if name == "--cuda-install":
        from voiceweave_worker.cuda_install import main as tool_main
    else:
        from voiceweave_worker.model_install import main as tool_main
    return tool_main()


def main() -> int:
    if len(sys.argv) > 1 and sys.argv[1] in _TOOLS:
        return _run_tool(sys.argv[1], sys.argv[2:])
    os.environ["HF_HUB_OFFLINE"] = "1"
    os.environ["TRANSFORMERS_OFFLINE"] = "1"
    _send("hello", None, worker_version=__version__)
    if "--self-check" in sys.argv:
        return 0
    model = None
    model_path = None
    # The English recognizer (Parakeet English), loaded next to `model` when the English
    # shortcut is on; used only by requests that say language "en".
    english_model = None
    while True:
        try:
            frame = read_frame(sys.stdin.buffer)
        except ProtocolError:
            # The stream is no longer trustworthy. Never print payload or raw text.
            return 2
        if frame is None:
            return 0
        header, payload = frame
        kind = header["kind"]
        request_id = header["request_id"]
        if request_id is None:
            return 2
        if kind == "shutdown":
            _send("bye", request_id)
            return 0
        try:
            if kind == "prepare" and header.get("engine", "whisper") != "whisper":
                # Parakeet (experimental): sherpa-onnx on the CPU.
                if payload:
                    raise ValueError("unexpected payload")
                from voiceweave_worker import sherpa_engine

                engine = header.get("engine")
                path = Path(header["model_dir"])
                if engine not in sherpa_engine.ENGINES:
                    raise ValueError("engine")
                if not path.is_absolute() or not sherpa_engine.files_present(engine, path):
                    _send("error", request_id, code="MODEL_MISSING")
                    continue
                model = sherpa_engine.load(engine, path, threads=header.get("cpu_threads", 4))
                import numpy as np

                model.warm_up()
                try:
                    # Recognition while speaking finds pauses with the same VAD as Whisper.
                    from faster_whisper.vad import get_speech_timestamps

                    get_speech_timestamps(np.zeros(16_000, dtype=np.float32))
                except Exception:
                    pass
                model_path = str(path)
                resident = _keep_resident()
                _send("ready", request_id, model_dir=model_path, device="cpu", engine=engine,
                      compute_type="int8",
                      resident_mb=None if resident is None else resident // (1024 * 1024))
            elif kind == "prepare_english":
                if payload:
                    raise ValueError("unexpected payload")
                from voiceweave_worker import sherpa_engine

                path = Path(header["model_dir"])
                if not path.is_absolute() or not sherpa_engine.files_present("nemo_transducer", path):
                    _send("error", request_id, code="MODEL_MISSING")
                    continue
                import numpy as np

                try:
                    english_model = sherpa_engine.load("nemo_transducer", path, threads=header.get("cpu_threads", 4))
                    english_model.warm_up()
                except (ImportError, OSError, RuntimeError, ValueError, TypeError, KeyError):
                    # The English recognizer is optional: only it is given up. The main
                    # model stays loaded (the shared handler below would drop it too).
                    english_model = None
                    _send("error", request_id, code="WORKER_OPERATION_FAILED")
                    continue
                resident = _keep_resident()
                _send("ready", request_id, model_dir=str(path), device="cpu", engine="nemo_transducer",
                      compute_type="int8",
                      resident_mb=None if resident is None else resident // (1024 * 1024))
            elif kind == "prepare":
                if payload:
                    raise ValueError("unexpected payload")
                path = Path(header["model_dir"])
                if not path.is_absolute() or not (path / "model.bin").is_file():
                    _send("error", request_id, code="MODEL_MISSING")
                    continue
                device = header.get("device", "cpu")
                if device not in ("cpu", "cuda"):
                    raise ValueError("device")
                if device == "cuda":
                    _enable_cuda_libraries(header.get("cuda_dir"))
                from faster_whisper import WhisperModel

                model = WhisperModel(
                    str(path),
                    device=device,
                    compute_type=header.get("compute_type", "int8"),
                    cpu_threads=header.get("cpu_threads", 4),
                    local_files_only=True,
                )
                # VAD would skip all-zero audio. Disable it only for this readiness
                # check so encoder/decoder failures occur before announcing Ready.
                # Any synthetic recognition output is discarded, never delivered.
                import numpy as np

                warmup_segments, _ = model.transcribe(
                    np.zeros(16_000, dtype=np.float32),
                    language="ja",
                    task="transcribe",
                    beam_size=1,
                    temperature=0,
                    condition_on_previous_text=False,
                    vad_filter=False,
                    word_timestamps=False,
                    max_new_tokens=1,
                )
                for _ in warmup_segments:
                    pass
                try:
                    # The VAD model otherwise loads on the first real recording (~3 s on
                    # this PC). Load it now; failure only costs that later delay.
                    from faster_whisper.vad import get_speech_timestamps

                    get_speech_timestamps(np.zeros(16_000, dtype=np.float32))
                except Exception:
                    pass
                model_path = str(path)
                # Stay in memory when other programs crowd the PC (best effort: a
                # failure here must never stop the recognizer from being ready).
                resident = _keep_resident()
                _send(
                    "ready",
                    request_id,
                    model_dir=model_path,
                    device=device,
                    compute_type=header.get("compute_type", "int8"),
                    resident_mb=None if resident is None else resident // (1024 * 1024),
                )
            elif kind == "stream_step":
                if model is None:
                    _send("error", request_id, code="MODEL_NOT_READY")
                    continue
                count = header.get("sample_count")
                if header.get("sample_rate") != 16000 or type(count) is not int or count <= 0                         or count * 4 != len(payload):
                    _send("error", request_id, code="INVALID_AUDIO_LENGTH")
                    continue
                import numpy as np

                from voiceweave_worker.streaming import stream_step

                audio = np.frombuffer(payload, dtype="<f4")
                if not bool(np.isfinite(audio).all()) or bool((np.abs(audio) > 1).any()):
                    _send("error", request_id, code="INVALID_AUDIO_VALUES")
                    continue
                hints = "、".join(header.get("dictionary_hints", []))
                context = str(header.get("context", ""))[-200:]
                prompt = (hints + ("、" if hints and context else "") + context) or None
                from voiceweave_worker.topics import parse, topic_terms, with_topic

                started = time.perf_counter()
                if hasattr(model, "use_hints"):
                    model.use_hints(list(header.get("dictionary_hints", [])))
                cut, text, reject = stream_step(model, audio, prompt=prompt)
                # A second pass with the topic's terms when the part names a topic.
                terms = topic_terms(text, parse(header.get("topics")), hints) \
                    if text and not hasattr(model, "use_hints") else []
                if terms:
                    again = stream_step(model, audio, prompt=with_topic(hints, terms, context))
                    if again[0] == cut and again[2] is None and again[1]:
                        text = again[1]
                    else:
                        terms = []
                _send("stream_result", request_id, session_id=header.get("session_id"), cut=cut,
                      text=text, reject=reject, topic_terms=len(terms),
                      elapsed_ms=round((time.perf_counter() - started) * 1000))
            elif kind == "transcribe":
                if model is None:
                    _send("error", request_id, code="MODEL_NOT_READY")
                    continue
                if header.get("sample_rate") != 16000 or header.get("channels") != 1:
                    _send("error", request_id, code="INVALID_AUDIO_FORMAT")
                    continue
                count = header.get("sample_count")
                if type(count) is not int or count <= 0 or count * 4 != len(payload):
                    _send("error", request_id, code="INVALID_AUDIO_LENGTH")
                    continue
                import numpy as np

                audio = np.frombuffer(payload, dtype="<f4")
                if not bool(np.isfinite(audio).all()) or bool((np.abs(audio) > 1).any()):
                    _send("error", request_id, code="INVALID_AUDIO_VALUES")
                    continue
                if header.get("language") == "en":
                    # The English shortcut: the whole recording, no hints. Whisper is
                    # multilingual, so it listens in English itself; Parakeet Japanese
                    # cannot, and uses Parakeet English loaded next to it.
                    started = time.perf_counter()
                    prepare_ms = None
                    if english_model is not None:
                        raw_text = english_model.recognize(audio)
                        prepare_ms = english_model.prepare_ms
                    elif not hasattr(model, "use_hints"):
                        # Without a punctuated example Whisper wrote English in lower case
                        # with no marks at all (turbo, 2026-10-29); with one it writes both.
                        segments, _ = model.transcribe(
                            audio, language="en", task="transcribe", beam_size=1, temperature=0,
                            condition_on_previous_text=False, vad_filter=True,
                            initial_prompt=ENGLISH_PROMPT)
                        raw_text = "".join(segment.text for segment in segments).strip()
                    else:
                        _send("error", request_id, code="MODEL_NOT_READY")
                        continue
                    elapsed = round((time.perf_counter() - started) * 1000)
                    _send("result", request_id, session_id=header.get("session_id"),
                          raw_text=raw_text, language="en", recognition_path="full", topic_terms=0,
                          elapsed_ms=elapsed,
                          **({} if prepare_ms is None
                             else {"frontend_ms": prepare_ms, "decode_ms": elapsed - prepare_ms}))
                    _keep_resident()
                    continue
                beam_size = header.get("beam_size", 1)
                hints = "、".join(header.get("dictionary_hints", []))
                # Recognized text before this audio (recognition while speaking).
                context = str(header.get("context", ""))[-200:]
                initial_prompt = (hints + ("、" if hints and context else "") + context) or None
                fast = header.get("fast_window", True) is not False

                def recognize(initial_prompt):
                    started = time.perf_counter()
                    short_text, details = None, {}
                    if fast:
                        try:
                            short_text, details = transcribe_short_window(
                                model, audio, beam_size=beam_size, initial_prompt=initial_prompt)
                        except Exception:
                            # Any doubt about the fast path means the full path decides.
                            short_text, details = None, {"reject": "exception"}
                    extra = {}
                    if short_text is not None:
                        timings = {
                            "elapsed_ms": round((time.perf_counter() - started) * 1000),
                            "frontend_ms": details["prepare_ms"],
                            "decode_ms": details["generate_ms"],
                        }
                        return short_text, "ja", "short", timings, extra
                    raw_text, info, timings = transcribe_measured(
                        model, audio, beam_size=beam_size, initial_prompt=initial_prompt)
                    path = "short_fallback" if fast else "full"
                    if fast:
                        # The total includes the abandoned fast attempt; the full path's
                        # parts would no longer add up to it, so report the total only.
                        timings = {"elapsed_ms": round((time.perf_counter() - started) * 1000)}
                        extra = {"fallback_reason": details.get("reject", "exception")}
                    return raw_text, info.language, path, timings, extra

                from voiceweave_worker.topics import parse, topic_terms, with_topic

                if hasattr(model, "use_hints"):
                    # Parakeet: the whole audio, cut at pauses when long.
                    started = time.perf_counter()
                    model.use_hints(list(header.get("dictionary_hints", [])))
                    raw_text = model.recognize(audio)
                    elapsed = round((time.perf_counter() - started) * 1000)
                    # The voice detector's time and the rest (the recognizer) add up to the total.
                    _send("result", request_id, session_id=header.get("session_id"),
                          raw_text=raw_text, language="ja", recognition_path="full", topic_terms=0,
                          elapsed_ms=elapsed, frontend_ms=model.prepare_ms,
                          decode_ms=elapsed - model.prepare_ms)
                    _keep_resident()
                    continue
                started = time.perf_counter()
                raw_text, language, path, timings, extra = recognize(initial_prompt)
                # A second pass with the topic's terms when the text names a topic.
                terms = topic_terms(raw_text, parse(header.get("topics")), hints) if raw_text else []
                if terms:
                    again = recognize(with_topic(hints, terms, context))
                    if again[0].strip():
                        raw_text, language, path, _, extra = again
                        # Two passes: their parts no longer add up, so the total only.
                        timings = {"elapsed_ms": round((time.perf_counter() - started) * 1000)}
                    else:
                        terms = []
                extra["topic_terms"] = len(terms)
                _send(
                    "result",
                    request_id,
                    session_id=header.get("session_id"),
                    raw_text=raw_text,
                    language=language,
                    recognition_path=path,
                    **extra,
                    **timings,
                )
                # Real recognition touches more memory than the readiness check; keep
                # that in memory too (after the reply, so it never delays the text).
                _keep_resident()
            else:
                _send("error", request_id, code="UNKNOWN_COMMAND")
        except (ImportError, OSError, RuntimeError, ValueError, TypeError, KeyError):
            # The parent decides whether to restart with a CPU configuration.
            model = None
            model_path = None
            _send("error", request_id, code="WORKER_OPERATION_FAILED")


if __name__ == "__main__":
    raise SystemExit(main())
