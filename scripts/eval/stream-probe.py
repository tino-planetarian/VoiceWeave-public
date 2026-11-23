"""Feed a long synthetic recording to the worker's stream_step one second at a time, as
the app does while recording, and report the wait after stop and the error rate.

Usage: python scripts/eval/stream-probe.py --audio-dir DIR --model-dir DIR --cuda-dir DIR
           [--seconds 60] [--condition normal]
Synthetic speech only (make-asr-audio.ps1); nothing is stored.
"""
from __future__ import annotations

import argparse
import importlib.util
import os
import sys
import time
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1] / "worker"))
spec = importlib.util.spec_from_file_location("eval_streaming", HERE / "eval-streaming.py")
es = importlib.util.module_from_spec(spec)
spec.loader.exec_module(es)
RATE = 16000
MIN_PENDING = 3.6  # same as the app (streaming.rs)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--audio-dir", type=Path, required=True)
    parser.add_argument("--model-dir", required=True)
    parser.add_argument("--cuda-dir", required=True)
    parser.add_argument("--seconds", type=float, default=60)
    parser.add_argument("--condition", default="normal")
    args = parser.parse_args()
    os.add_dll_directory(args.cuda_dir)
    os.environ["PATH"] = args.cuda_dir + os.pathsep + os.environ["PATH"]
    from faster_whisper import WhisperModel
    from voiceweave_worker.streaming import stream_step
    from voiceweave_worker.short_window import transcribe_short_window
    from voiceweave_worker.timing import transcribe_measured

    model = WhisperModel(args.model_dir, device="cuda", compute_type="int8_float16", local_files_only=True)
    es.recognize(model, np.zeros(RATE, dtype=np.float32), None)
    audio, reference = es.build(args.audio_dir, args.condition, args.seconds, 0.5 if args.condition == "normal" else 0.2)
    committed, texts, clock, failed, steps = 0, [], 0.0, False, 0
    # Simulated time: a step starts once a second's audio has arrived and the worker is free.
    for second in range(1, int(len(audio) / RATE) + 1):
        now = float(second)
        if now < clock or failed:
            continue
        pending = audio[committed:second * RATE]
        if len(pending) / RATE < MIN_PENDING:
            continue
        started = time.perf_counter()
        cut, text, reject = stream_step(model, pending, prompt="".join(texts)[-120:] or None)
        spent = time.perf_counter() - started
        clock = now + spent
        steps += 1
        if reject:
            failed = True
        elif cut is not None:
            committed += cut
            texts.append(text)
    stop = len(audio) / RATE
    started = time.perf_counter()
    tail = audio[committed:]
    if failed or not texts:
        text, _, _ = transcribe_measured(model, audio, beam_size=1, initial_prompt=None)
        final = text
    else:
        tail_text = ""
        if len(tail) >= RATE // 2:
            tail_text, _ = transcribe_short_window(model, tail, beam_size=1, initial_prompt="".join(texts)[-120:])
            if tail_text is None:
                tail_text, _, _ = transcribe_measured(model, tail, beam_size=1, initial_prompt="".join(texts)[-120:])
        final = "".join(texts) + tail_text
    wait = max(0.0, clock - stop) + (time.perf_counter() - started)
    print(f"{args.condition} {stop:.1f}s: steps {steps}, parts {len(texts)}, failed {failed}, tail {len(tail)/RATE:.1f}s, "
          f"wait after stop {wait:.2f}s, CER {es.cer(reference, final):.3f}")


if __name__ == "__main__":
    main()
