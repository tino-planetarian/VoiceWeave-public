"""Punctuation candidate: a local model (Ollama) inserts only 、/。, and its answer is
used only when removing all punctuation from it gives back exactly the input characters
(the "never rewrite words" rule). Otherwise the input is kept unchanged.

Usage: python scripts/eval/punct-llm.py --results eval.json --model gemma3:4b [--source raw|app]
Reads the per-file texts written by eval-asr.py, punctuates them, and prints the same
punctuation metrics as eval-asr.py for the source texts and the candidate. Run it with
the recognition model unloaded (it uses the GPU through Ollama).
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import statistics
import time
import urllib.request
from pathlib import Path

spec = importlib.util.spec_from_file_location("eval_asr", Path(__file__).with_name("eval-asr.py"))
eval_asr = importlib.util.module_from_spec(spec)
spec.loader.exec_module(eval_asr)

PROMPT = """次の日本語の文に、読みやすくなるよう読点「、」と句点「。」を補ってください。
規則:
- 文字は一字も変えない・足さない・消さない・並べ替えない。補うのは「、」と「。」だけ。
- すでにある句読点は、不要なら取り除いてよい。
- 出力は句読点を補った文だけ。説明は書かない。"""


def bare(text: str) -> str:
    return eval_asr.strip_with_marks(text)[0]


def punctuate(host: str, model: str, text: str) -> tuple[str, bool, float]:
    body = json.dumps({
        "model": model,
        "messages": [{"role": "system", "content": PROMPT}, {"role": "user", "content": text}],
        "stream": False, "think": False, "keep_alive": "10m",
        "options": {"temperature": 0, "num_ctx": 2048},
    }).encode()
    request = urllib.request.Request(f"{host}/api/chat", data=body, headers={"Content-Type": "application/json"})
    started = time.perf_counter()
    with urllib.request.urlopen(request, timeout=120) as response:
        answer = json.load(response)["message"]["content"].strip()
    seconds = time.perf_counter() - started
    # The safety check: the same characters in the same order, only marks differ.
    if bare(answer) == bare(text):
        return answer, True, seconds
    return text, False, seconds


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--results", type=Path, required=True)
    parser.add_argument("--model", default="gemma3:4b")
    parser.add_argument("--source", default="app", choices=["raw", "app"])
    parser.add_argument("--config", type=int, default=0, help="index of the eval-asr configuration")
    parser.add_argument("--host", default="http://127.0.0.1:11434")
    parser.add_argument("--out", type=Path)
    args = parser.parse_args()
    references = {}
    for line in Path(__file__).with_name("asr_eval_ja.tsv").read_text(encoding="utf-8").splitlines():
        if line.strip():
            sentence_id, text = line.split("\t", 1)
            references[sentence_id] = text
    config = json.loads(args.results.read_text(encoding="utf-8"))[args.config]
    punctuate(args.host, args.model, "テストです")  # load the model, not timed
    rows, times, accepted = [], [], 0
    for row in config["rows"]:
        text, ok, seconds = punctuate(args.host, args.model, row[args.source])
        accepted += ok
        times.append(seconds)
        rows.append({**row, "llm": text, "llm_ok": ok})
    summary = {"model": args.model, "source": args.source, "asr": {k: config[k] for k in ("model", "beam", "prompt")},
               "accepted": f"{accepted}/{len(rows)}", "median_s": round(statistics.median(times), 3),
               "max_s": round(max(times), 3), "conditions": {}}
    for condition in sorted({row["condition"] for row in rows}):
        subset = [row for row in rows if row["condition"] == condition]
        line = {}
        for key in (args.source, "llm"):
            counts = {name: 0 for name in ("ref", "hyp", "hit", "same_mark")}
            for row in subset:
                for name, value in eval_asr.punctuation_counts(references[row["id"]], row[key]).items():
                    counts[name] += value
            line[key] = {"position": eval_asr.f1(counts["hit"], counts["hyp"], counts["ref"]),
                         "same_mark": eval_asr.f1(counts["same_mark"], counts["hyp"], counts["ref"])}
        summary["conditions"][condition] = line
    print(json.dumps(summary, ensure_ascii=False))
    if args.out:
        args.out.write_text(json.dumps({"summary": summary, "rows": rows}, ensure_ascii=False, indent=1), encoding="utf-8")


if __name__ == "__main__":
    main()
