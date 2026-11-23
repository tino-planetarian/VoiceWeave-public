"""Compare the full 30 s window with the shortened-window fast path on synthetic speech.

Usage: python scripts/eval/eval-window.py --audio-dir DIR --model-dir DIR [--cuda-dir DIR]
Audio comes from scripts/eval/make-audio.ps1. Prints only aggregate error rates and
timings plus differing sentences, which are synthetic text from sentences_ja.txt.
"""
from __future__ import annotations

import argparse
import os
import statistics
import time
import unicodedata
import wave
from pathlib import Path

import numpy as np

PUNCTUATION = set("、。，．・！？!?「」『』（）() 　")


def normalize(text: str) -> str:
    text = unicodedata.normalize("NFKC", text)
    return "".join(c for c in text if c not in PUNCTUATION and not c.isspace())


def edit_distance(a: str, b: str) -> int:
    row = list(range(len(b) + 1))
    for i, ca in enumerate(a, 1):
        prev, row[0] = row[0], i
        for j, cb in enumerate(b, 1):
            prev, row[j] = row[j], min(row[j] + 1, row[j - 1] + 1, prev + (ca != cb))
    return row[-1]


def load(path: Path) -> np.ndarray:
    with wave.open(str(path)) as w:
        return (np.frombuffer(w.readframes(w.getnframes()), "<i2").astype(np.float32) / 32768).copy()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--audio-dir", type=Path, required=True)
    parser.add_argument("--model-dir", required=True)
    parser.add_argument("--cuda-dir")
    parser.add_argument("--min-frames", default="1000,1500")
    args = parser.parse_args()
    if args.cuda_dir:
        os.add_dll_directory(args.cuda_dir)
        os.environ["PATH"] = args.cuda_dir + os.pathsep + os.environ["PATH"]
    from faster_whisper import WhisperModel
    from voiceweave_worker.short_window import transcribe_short_window

    model = WhisperModel(args.model_dir, device="cuda" if args.cuda_dir else "cpu",
                         compute_type="int8_float16" if args.cuda_dir else "int8",
                         local_files_only=True)
    sentences = (Path(__file__).with_name("sentences_ja.txt")
                 .read_text(encoding="utf-8").split("\n"))
    sentences = [s for s in sentences if s.strip()]
    files = sorted(args.audio_dir.glob("r*_*.wav"))

    def full(audio):
        segments, _ = model.transcribe(audio, language="ja", beam_size=1, temperature=0,
                                       condition_on_previous_text=False, vad_filter=True)
        return "".join(s.text for s in segments)

    reasons: dict[str, int] = {}
    methods = {"full(current)": lambda a: (full(a), False)}
    for minimum in [int(x) for x in args.min_frames.split(",")]:
        def short(a, minimum=minimum):
            text, details = transcribe_short_window(model, a, beam_size=1, initial_prompt=None,
                                                    min_frames=minimum)
            if text is None:
                reasons[details["reject"]] = reasons.get(details["reject"], 0) + 1
                return full(a), True
            return text, False
        methods[f"short(min{minimum})+fallback"] = short

    full(load(files[0]))  # warm-up, not measured
    for name, run in methods.items():
        errors = chars = fallbacks = 0
        times: list[float] = []
        diffs: list[str] = []
        for path in files:
            index = int(path.stem.split("_")[1])
            reference = normalize(sentences[index])
            audio = load(path)
            started = time.perf_counter()
            text, fell_back = run(audio)
            times.append((time.perf_counter() - started) * 1000)
            fallbacks += fell_back
            distance = edit_distance(normalize(text), reference)
            errors += distance
            chars += len(reference)
            if distance:
                diffs.append(f"    {path.stem} err={distance}: {text}")
        print(f"{name:26} CER={errors / chars:.2%} ({errors}/{chars}) "
              f"median={statistics.median(times):.0f}ms p90={sorted(times)[int(len(times)*0.9)]:.0f}ms "
              f"fallback={fallbacks}/{len(files)}")
        for line in diffs:
            print(line)
    print("fallback reasons:", reasons)


if __name__ == "__main__":
    main()
