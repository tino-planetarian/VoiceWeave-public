"""Synthetic recognition benchmark. Never stores audio or recognised text.

Usage: python scripts/bench-transcribe.py --wav A.wav [--wav B.wav] --model-dir DIR
       [--threads 4,8] [--compute int8] [--beam 1] [--repeat 3] [--no-vad]

Prints only timings, token counts and text length. WAV must be 16 kHz mono 16-bit.
"""
from __future__ import annotations

import argparse
import statistics
import time
import wave

import numpy as np


def load_wav(path: str) -> np.ndarray:
    with wave.open(path, "rb") as w:
        assert w.getframerate() == 16000 and w.getnchannels() == 1 and w.getsampwidth() == 2
        data = np.frombuffer(w.readframes(w.getnframes()), dtype="<i2")
    return (data.astype(np.float32) / 32768.0).copy()


def run(model, audio, *, beam: int, vad: bool, extra: dict):
    t0 = time.perf_counter()
    segments, _info = model.transcribe(
        audio, language="ja", task="transcribe", beam_size=beam, temperature=0,
        condition_on_previous_text=False, vad_filter=vad, word_timestamps=False, **extra,
    )
    t1 = time.perf_counter()
    text = "".join(s.text for s in segments)
    t2 = time.perf_counter()
    return (t1 - t0) * 1000, (t2 - t1) * 1000, len(text)


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--wav", action="append", required=True)
    p.add_argument("--model-dir", required=True)
    p.add_argument("--threads", default="4")
    p.add_argument("--compute", default="int8")
    p.add_argument("--beam", type=int, default=1)
    p.add_argument("--repeat", type=int, default=3)
    p.add_argument("--device", default="cpu")
    p.add_argument("--no-vad", action="store_true")
    p.add_argument("--num-workers", type=int, default=1)
    args = p.parse_args()
    from faster_whisper import WhisperModel

    audios = {w: load_wav(w) for w in args.wav}
    for threads in [int(t) for t in args.threads.split(",")]:
        t0 = time.perf_counter()
        model = WhisperModel(args.model_dir, device=args.device, compute_type=args.compute,
                             cpu_threads=threads, num_workers=args.num_workers,
                             local_files_only=True)
        load_ms = (time.perf_counter() - t0) * 1000
        # Same warm-up as the worker's prepare step.
        w0 = time.perf_counter()
        for _ in model.transcribe(np.zeros(16000, np.float32), language="ja", beam_size=1,
                                  vad_filter=False, max_new_tokens=1)[0]:
            pass
        warm_ms = (time.perf_counter() - w0) * 1000
        print(f"[threads={threads} compute={args.compute} beam={args.beam} vad={not args.no_vad}] "
              f"load={load_ms:.0f}ms warmup={warm_ms:.0f}ms")
        for name, audio in audios.items():
            rows = [run(model, audio, beam=args.beam, vad=not args.no_vad, extra={})
                    for _ in range(args.repeat)]
            fe = [r[0] for r in rows]
            dc = [r[1] for r in rows]
            print(f"  audio={len(audio)/16000:.2f}s chars={rows[-1][2]} "
                  f"run1 total={fe[0]+dc[0]:.0f}ms | "
                  f"median frontend={statistics.median(fe):.0f} decode={statistics.median(dc):.0f} "
                  f"total={statistics.median([a+b for a,b in zip(fe,dc)]):.0f}ms "
                  f"all={[round(a+b) for a,b in zip(fe,dc)]}")
        del model


if __name__ == "__main__":
    main()
