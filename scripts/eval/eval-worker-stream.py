"""Recognition while speaking through the real worker, with and without glossary topics.

Usage:
  python scripts/eval/eval-worker-stream.py --audio-dir DIR --model-dir DIR --cuda-dir DIR
      [--condition normal] [--gap 0.7] [--align PATH]
  python scripts/eval/eval-worker-stream.py --audio-dir DIR --model-dir DIR --device cpu
      [--threads 4] [--modes none,off]   (as the app runs without a GPU: int8, 4 threads;
      "off" = recognition while speaking turned off)

Joins the synthetic astronomy files (make-asr-audio.ps1 -Tsv astro_eval_ja.tsv) into one
long recording with short silences, then plays it like the app: every second the audio
not yet recognized goes to the worker as "stream_step" (with the recognized text as
context); at the end the rest goes as "transcribe". Reports the worker's busy time per
step (it must stay under the one-second step to keep up), the wait after stop (the last
"transcribe"), parts recognized again with topics, and the share of glossary terms
written right. Synthetic speech only.

"wait_realtime_s" estimates the wait after stop when the steps could not keep up: steps
run one after another, each starting when its second of audio exists and the previous
step is done, so time a slow step takes is added to the wait (the app would cut longer
parts instead; this is an approximation).
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import subprocess
import sys
import time
import uuid
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("worker_topics", HERE / "eval-worker-topics.py")
worker_topics = importlib.util.module_from_spec(spec)
spec.loader.exec_module(worker_topics)
eval_terms = worker_topics.eval_terms
read_frame = worker_topics.read_frame
request = worker_topics.request
RATE = 16000


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--audio-dir", type=Path, required=True)
    parser.add_argument("--model-dir", required=True)
    parser.add_argument("--cuda-dir")
    parser.add_argument("--device", choices=("cuda", "cpu"), default="cuda")
    parser.add_argument("--threads", type=int, default=4)
    parser.add_argument("--modes", default="none,topics")
    parser.add_argument("--sentences", type=int, default=0, help="use only the first N (0: all 26)")
    parser.add_argument("--condition", default="normal")
    parser.add_argument("--gap", type=float, default=0.7)
    parser.add_argument("--align")
    parser.add_argument("--cores", type=int, default=0,
                        help="run on only the first N logical processors (a slower PC, roughly); 0: all")
    args = parser.parse_args()
    if args.cores:
        # Inherited by the worker started below.
        import ctypes
        kernel32 = ctypes.windll.kernel32
        kernel32.GetCurrentProcess.restype = ctypes.c_void_p
        assert kernel32.SetProcessAffinityMask(ctypes.c_void_p(kernel32.GetCurrentProcess()),
                                               ctypes.c_size_t((1 << args.cores) - 1))

    glossary = eval_terms.glossary_terms()
    topics = eval_terms.glossary_lines()
    references = []
    pieces = []
    silence = np.zeros(int(args.gap * RATE), dtype=np.float32)
    lines = [line for line in (HERE / "astro_eval_ja.tsv").read_text(encoding="utf-8").splitlines() if line.strip()]
    for line in lines[:args.sentences or None]:
        if line.strip():
            key, text = line.split("\t")[:2]
            references.append(text)
            pieces += [eval_terms.eval_asr.load(args.audio_dir / f"{args.condition}_{key}.wav"), silence]
    audio = np.concatenate(pieces)
    expected = [term for text in references for term in eval_terms.expected_terms(text, glossary)]

    worker = subprocess.Popen([sys.executable, "-m", "voiceweave_worker"], cwd=HERE.parents[1] / "worker",
                              stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    assert read_frame(worker.stdout)[0]["kind"] == "hello"
    if args.device == "cuda":
        prepare = {"device": "cuda", "cuda_dir": args.cuda_dir, "compute_type": "int8_float16"}
    else:
        prepare = {"device": "cpu", "compute_type": "int8", "cpu_threads": args.threads}
    loading = time.perf_counter()
    ready = request(worker, {"kind": "prepare", "model_dir": args.model_dir, **prepare})
    print(json.dumps({"device": args.device, "load_s": round(time.perf_counter() - loading, 1)}), flush=True)
    assert ready["kind"] == "ready", ready
    try:
        for mode in args.modes.split(","):
            sent_topics = topics if mode == "topics" else []
            committed, texts, busy, passes = 0, [], [], 0
            clock = 0.0  # simulated time at which the worker is free again
            # "off": recognition while speaking turned off, the whole recording after stop.
            for end in range(RATE, len(audio) + 1, RATE) if mode != "off" else ():
                pending = audio[committed:end]
                started = time.perf_counter()
                reply = request(worker, {
                    "kind": "stream_step", "session_id": str(uuid.uuid4()), "sample_rate": RATE,
                    "channels": 1, "sample_count": len(pending), "dictionary_hints": [],
                    "context": "".join(texts)[-200:], "topics": sent_topics,
                }, pending.astype("<f4").tobytes())
                busy.append(time.perf_counter() - started)
                clock = max(clock, end / RATE) + busy[-1]
                if reply.get("cut") is not None and reply.get("reject") is None:
                    committed += reply["cut"]
                    texts.append(reply["text"])
                    passes += reply.get("topic_terms", 0) > 0
            tail = audio[committed:]
            started = time.perf_counter()
            reply = request(worker, {
                "kind": "transcribe", "session_id": str(uuid.uuid4()), "sample_rate": RATE, "channels": 1,
                "sample_count": len(tail), "beam_size": 1, "dictionary_hints": [],
                "context": "".join(texts)[-200:], "fast_window": True, "topics": sent_topics,
            }, tail.astype("<f4").tobytes())
            wait = time.perf_counter() - started
            wait_realtime = max(clock, len(audio) / RATE) + wait - len(audio) / RATE
            texts.append(reply["raw_text"])
            passes += reply.get("topic_terms", 0) > 0
            text = "".join(texts)
            if args.align:
                text = subprocess.run([str(Path(args.align).resolve())], input=(text.replace("\n", " ") + "\n").encode("utf-8"),
                                      capture_output=True, check=True).stdout.decode("utf-8").strip()
            right = sum(eval_terms.clean(term) in eval_terms.clean(text) for term in expected)
            print(json.dumps({
                "mode": mode, "seconds": round(len(audio) / RATE, 1), "parts": len(texts) - 1,
                "topic_passes": passes, "term_accuracy_by_presence": round(right / len(expected), 3),
                "step_busy_median_s": round(float(np.median(busy)), 3) if busy else None,
                "step_busy_max_s": round(max(busy), 3) if busy else None,
                "steps_over_1s": sum(b > 1.0 for b in busy), "wait_after_stop_s": round(wait, 3),
                "wait_realtime_s": round(wait_realtime, 2),
            }, ensure_ascii=False), flush=True)
    finally:
        request(worker, {"kind": "shutdown"})
        worker.wait(timeout=30)


if __name__ == "__main__":
    main()
