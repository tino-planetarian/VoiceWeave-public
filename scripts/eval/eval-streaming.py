"""Prototype: recognize while the user is still speaking, and measure the wait after stop.

Usage: python scripts/eval/eval-streaming.py --audio-dir DIR --model turbo=DIR [--model ...]
           --cuda-dir DIR [--lengths 30,60,120] [--conditions normal,nopause] [--out FILE]

Long recordings are built by joining the evaluation sentences (synthetic speech from
make-asr-audio.ps1). Two ways of recognizing the same recording are compared:

- "batch" (today's app): after stop, the whole recording goes through the app path
  (short window up to 25 s, otherwise the 30 s-window full path).
- "streaming": while the recording plays in (simulated) real time, each stretch that
  ends at a natural pause is recognized in the background; at stop only the remaining
  tail is recognized. The previous text is passed as context. The wait after stop is
  the time until the background queue and the tail are done.

Reported: wait after stop (seconds) and character error rate, per length and model.
Synthetic speech only; nothing personal is read or stored.
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import os
import sys
import time
import wave
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1] / "worker"))
spec = importlib.util.spec_from_file_location("eval_asr", HERE / "eval-asr.py")
eval_asr = importlib.util.module_from_spec(spec)
spec.loader.exec_module(eval_asr)

RATE = 16000


def load(path: Path) -> np.ndarray:
    with wave.open(str(path)) as w:
        return np.frombuffer(w.readframes(w.getnframes()), "<i2").astype(np.float32) / 32768


def build(audio_dir: Path, condition: str, seconds: float, gap: float) -> tuple[np.ndarray, str]:
    """Join sentences (cycling through the set) until the recording is long enough."""
    references = [line.split("\t", 1) for line in (HERE / "asr_eval_ja.tsv").read_text(encoding="utf-8").splitlines() if line.strip()]
    parts, text, total, index = [], [], 0.0, 0
    silence = np.zeros(int(gap * RATE), dtype=np.float32)
    while total < seconds:
        sentence_id, reference = references[index % len(references)]
        clip = load(audio_dir / f"{condition}_{sentence_id}.wav")
        parts += [clip, silence]
        text.append(reference)
        total += (len(clip) + len(silence)) / RATE
        index += 1
    return np.concatenate(parts), "".join(text)


def recognize(model, audio: np.ndarray, prompt: str | None) -> tuple[str, float]:
    from voiceweave_worker.short_window import transcribe_short_window
    from voiceweave_worker.timing import transcribe_measured

    started = time.perf_counter()
    text = None
    if len(audio) / RATE <= 25:
        try:
            text, _ = transcribe_short_window(model, audio, beam_size=1, initial_prompt=prompt)
        except Exception:
            text = None
    if text is None:
        text, _, _ = transcribe_measured(model, audio, beam_size=1, initial_prompt=prompt)
    return text, time.perf_counter() - started


def cut_points(audio: np.ndarray, min_chunk: float, max_chunk: float, min_gap: float) -> list[int]:
    """Sample positions to cut at: the middle of a pause, once a chunk is long enough.

    Only pauses the speaker has already finished are used, as a live recorder would.
    """
    from faster_whisper.vad import VadOptions, get_speech_timestamps

    # The default merges speech across pauses shorter than 2 s; a breath is ~0.2-0.5 s.
    speech = get_speech_timestamps(audio, VadOptions(min_silence_duration_ms=int(min_gap * 1000),
                                                     speech_pad_ms=50))
    gaps = [(a["end"], b["start"]) for a, b in zip(speech, speech[1:]) if (b["start"] - a["end"]) / RATE >= min_gap]
    cuts, last = [], 0
    for start, end in gaps:
        middle = (start + end) // 2
        if (middle - last) / RATE >= min_chunk:
            cuts.append(middle)
            last = middle
        elif (middle - last) / RATE >= max_chunk:
            cuts.append(middle)
            last = middle
    return cuts


def streaming(model, audio: np.ndarray, cuts: list[int], context_chars: int, notice_delay: float):
    """Simulate live capture: a chunk can start once its pause has been heard."""
    bounds = [0, *cuts, len(audio)]
    texts, free_at, details = [], 0.0, []
    for index in range(len(bounds) - 1):
        start, end = bounds[index], bounds[index + 1]
        is_tail = index == len(bounds) - 2
        ready = end / RATE + (0.0 if is_tail else notice_delay)
        prompt = "".join(texts)[-context_chars:] or None
        text, seconds = recognize(model, audio[start:end], prompt)
        begin = max(ready, free_at)
        free_at = begin + seconds
        texts.append(text.strip())
        details.append({"chunk_s": round((end - start) / RATE, 1), "proc_s": round(seconds, 3), "waited_s": round(begin - ready, 3)})
    wait = max(0.0, free_at - len(audio) / RATE)
    return "".join(texts), wait, details


def cer(reference: str, text: str) -> float:
    ref = eval_asr.normalize_style(eval_asr.strip_with_marks(reference)[0])
    hyp = eval_asr.normalize_style(eval_asr.strip_with_marks(text)[0])
    return eval_asr.edit_distance(hyp, ref) / len(ref)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--audio-dir", type=Path, required=True)
    parser.add_argument("--model", action="append", required=True)
    parser.add_argument("--cuda-dir", required=True)
    parser.add_argument("--lengths", default="30,60,120")
    parser.add_argument("--conditions", default="normal,nopause")
    parser.add_argument("--min-chunk", type=float, default=6.0)
    parser.add_argument("--max-chunk", type=float, default=20.0)
    parser.add_argument("--min-gap", type=float, default=0.25)
    parser.add_argument("--context-chars", type=int, default=120)
    parser.add_argument("--notice-delay", type=float, default=0.3, help="silence heard before a cut is trusted")
    parser.add_argument("--out", type=Path)
    args = parser.parse_args()
    os.add_dll_directory(args.cuda_dir)
    os.environ["PATH"] = args.cuda_dir + os.pathsep + os.environ["PATH"]
    from faster_whisper import WhisperModel

    results = []
    for spec_text in args.model:
        name, directory = spec_text.split("=", 1)
        model = WhisperModel(directory, device="cuda", compute_type="int8_float16", local_files_only=True)
        recognize(model, np.zeros(RATE, dtype=np.float32), None)  # warm-up
        for condition in args.conditions.split(","):
            # Sentences are separated by a short breath: longer for natural speech.
            gap = 0.5 if condition == "normal" else 0.2
            for seconds in [float(value) for value in args.lengths.split(",")]:
                audio, reference = build(args.audio_dir, condition, seconds, gap)
                batch_text, batch_wait = recognize(model, audio, None)
                cuts = cut_points(audio, args.min_chunk, args.max_chunk, args.min_gap)
                stream_text, stream_wait, details = streaming(model, audio, cuts, args.context_chars, args.notice_delay)
                row = {
                    "model": name, "condition": condition, "audio_s": round(len(audio) / RATE, 1),
                    "batch": {"wait_s": round(batch_wait, 2), "cer": round(cer(reference, batch_text), 4)},
                    "streaming": {"wait_s": round(stream_wait, 2), "cer": round(cer(reference, stream_text), 4),
                                  "chunks": len(details), "tail_s": details[-1]["chunk_s"],
                                  "max_queue_wait_s": max(d["waited_s"] for d in details)},
                }
                print(json.dumps(row, ensure_ascii=False), flush=True)
                results.append({**row, "details": details, "batch_text": batch_text, "stream_text": stream_text})
        del model
    if args.out:
        args.out.write_text(json.dumps(results, ensure_ascii=False, indent=1), encoding="utf-8")


if __name__ == "__main__":
    main()
