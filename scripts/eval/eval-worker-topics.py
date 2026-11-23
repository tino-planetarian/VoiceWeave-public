"""The topic second pass through the real worker process, as the app sends it.

Usage:
  python scripts/eval/eval-worker-topics.py --audio-dir DIR --model-dir DIR --cuda-dir DIR
      [--align PATH] [--out FILE.json]

Starts `python -m voiceweave_worker`, prepares the model like the app, and sends every
synthetic astronomy file (make-asr-audio.ps1 -Tsv astro_eval_ja.tsv) as a "transcribe"
request twice: without topics and with the default glossary's topics
(glossary::topics). Reports the share of glossary terms written right (after the app's
glossary spelling step with --align), how many files took the second pass, and the
worker's time. Audio is synthetic; no text is stored unless --out is given.
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import statistics
import subprocess
import sys
import uuid
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(ROOT / "worker"))
from voiceweave_worker.protocol import read_frame, write_frame  # noqa: E402

spec = importlib.util.spec_from_file_location("eval_terms", HERE / "eval-terms.py")
eval_terms = importlib.util.module_from_spec(spec)
spec.loader.exec_module(eval_terms)


def request(worker, header: dict, payload: bytes = b"") -> dict:
    write_frame(worker.stdin, {"request_id": str(uuid.uuid4()), **header}, payload)
    worker.stdin.flush()
    reply = read_frame(worker.stdout)
    if reply is None:
        raise RuntimeError("worker closed")
    return reply[0]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--audio-dir", type=Path, required=True)
    parser.add_argument("--model-dir", required=True)
    parser.add_argument("--cuda-dir", required=True)
    parser.add_argument("--tsv", default="astro_eval_ja.tsv", help="reference sentences")
    parser.add_argument("--glob", default="*_a*.wav")
    parser.add_argument("--align")
    parser.add_argument("--out", type=Path)
    args = parser.parse_args()

    glossary = eval_terms.glossary_terms()
    topics = eval_terms.glossary_lines()
    references = {}
    for line in (HERE / args.tsv).read_text(encoding="utf-8").splitlines():
        if line.strip():
            parts = line.split("\t")
            references[parts[0]] = parts[1]
    files = sorted(args.audio_dir.glob(args.glob))

    worker = subprocess.Popen([sys.executable, "-m", "voiceweave_worker"], cwd=ROOT / "worker",
                              stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    hello = read_frame(worker.stdout)
    assert hello and hello[0]["kind"] == "hello"
    ready = request(worker, {"kind": "prepare", "model_dir": args.model_dir, "device": "cuda",
                             "cuda_dir": args.cuda_dir, "compute_type": "int8_float16"})
    assert ready["kind"] == "ready", ready
    results = {}
    try:
        for mode in ("none", "topics"):
            rows = []
            for path in files:
                key = path.stem.split("_", 1)[1]
                audio = eval_terms.eval_asr.load(path)
                reply = request(worker, {
                    "kind": "transcribe", "session_id": str(uuid.uuid4()), "sample_rate": 16000,
                    "channels": 1, "sample_count": len(audio), "beam_size": 1,
                    "dictionary_hints": [], "context": "", "fast_window": True,
                    "topics": topics if mode == "topics" else [],
                }, audio.astype("<f4").tobytes())
                assert reply["kind"] == "result", reply
                rows.append({"file": path.stem, "id": key, "raw": reply["raw_text"],
                             "ms": reply["elapsed_ms"], "topic_terms": reply.get("topic_terms", 0),
                             "terms": eval_terms.expected_terms(references[key], glossary)})
            texts = [row["raw"] for row in rows]
            if args.align:
                done = subprocess.run([str(Path(args.align).resolve())],
                                      input=("\n".join(t.replace("\n", " ") for t in texts) + "\n").encode("utf-8"),
                                      capture_output=True, check=True)
                texts = done.stdout.decode("utf-8").split("\n")[: len(rows)]
            total = right = 0
            for row, text in zip(rows, texts):
                row["final"] = text
                row["missed"] = [t for t in row["terms"] if eval_terms.clean(t) not in eval_terms.clean(text)]
                total += len(row["terms"])
                right += len(row["terms"]) - len(row["missed"])
            errors = sum(eval_terms.eval_asr.edit_distance(
                eval_terms.eval_asr.normalize_style(eval_terms.clean(row["final"])),
                eval_terms.eval_asr.normalize_style(eval_terms.clean(references[row["id"]]))) for row in rows)
            length = sum(len(eval_terms.clean(references[row["id"]])) for row in rows)
            summary = {"mode": mode, "files": len(rows),
                       "term_accuracy": round(right / total, 3) if total else None,
                       "cer": round(errors / length, 4),
                       "second_pass": sum(row["topic_terms"] > 0 for row in rows),
                       "median_ms": statistics.median(row["ms"] for row in rows),
                       "max_ms": max(row["ms"] for row in rows)}
            print(json.dumps(summary, ensure_ascii=False), flush=True)
            results[mode] = {"summary": summary, "rows": rows}
    finally:
        request(worker, {"kind": "shutdown"})
        worker.wait(timeout=30)
    if args.out:
        args.out.write_text(json.dumps(results, ensure_ascii=False, indent=1), encoding="utf-8")


if __name__ == "__main__":
    main()
