"""Compare the two cloud drafting models on fixed material: time, term fixes, meaning checks.

Usage:
  python scripts/eval/polish-compare.py --terms-result FILE.json --tool PATH --out FILE.jsonl
      [--model openai=gpt-6-luna --model anthropic=NEW-MODEL ...]

Without --model, the app's default model of each service. A new model is compared with
the current one on the same material by naming both.

Material (synthetic, no personal speech):
- astro: the transcripts of scripts/eval/eval-terms.py (hints "none", after the app's
  glossary spelling step), with Whisper's mistakes in them. Score: how many of the
  sentence's glossary terms the draft has (the transcript's own rate is shown beside it).
- talk: scripts/eval/polish_samples_ja.json (spoken style with fillers and restarts).
  Score: every "keep" group has one of its words in the draft, no "drop" word remains.
The drafts go to --out (outside the repository); they are synthetic sentences.
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import statistics
import subprocess
from pathlib import Path

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("eval_terms", HERE / "eval-terms.py")
eval_terms = importlib.util.module_from_spec(spec)
spec.loader.exec_module(eval_terms)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--terms-result", type=Path, required=True)
    parser.add_argument("--tool", required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--model", action="append", default=[], help="service=model")
    args = parser.parse_args()

    terms_run = next(run for run in json.loads(args.terms_result.read_text(encoding="utf-8"))
                     if run["hints"] == "none")
    items = []
    for row in terms_run["rows"]:
        items.append({"id": f"astro_{row['condition']}_{row['id']}", "text": row.get("aligned", row["raw"]),
                      "terms": row["terms"]})
    talks = json.loads((HERE / "polish_samples_ja.json").read_text(encoding="utf-8"))
    for sample in talks:
        items.append({"id": f"talk_{sample['id']}", "text": sample["text"],
                      "keep": sample["keep"], "drop": sample["drop"]})

    lines = "".join(json.dumps({"id": item["id"], "text": item["text"]}, ensure_ascii=False) + "\n"
                    for item in items)
    done = subprocess.run([str(Path(args.tool).resolve()), *args.model], input=lines.encode("utf-8"),
                          capture_output=True, check=True)
    records = [json.loads(line) for line in done.stdout.decode("utf-8").splitlines() if line.strip()]
    args.out.write_text("".join(json.dumps(r, ensure_ascii=False) + "\n" for r in records), encoding="utf-8")

    by_id = {item["id"]: item for item in items}
    for service, model in sorted({(r["service"], r["model"]) for r in records}):
        mine = [r for r in records if r["service"] == service and r["model"] == model]
        times = sorted(r["seconds"] for r in mine if r["ok"])
        astro = [r for r in mine if r["id"].startswith("astro_") and r["ok"]]
        total = sum(len(by_id[r["id"]]["terms"]) for r in astro)
        before = sum(sum(eval_terms.clean(t) in eval_terms.clean(by_id[r["id"]]["text"])
                         for t in by_id[r["id"]]["terms"]) for r in astro)
        after = sum(sum(eval_terms.clean(t) in eval_terms.clean(r["text"])
                        for t in by_id[r["id"]]["terms"]) for r in astro)
        talk = [r for r in mine if r["id"].startswith("talk_") and r["ok"]]
        kept = dropped = 0
        for r in talk:
            item = by_id[r["id"]]
            kept += all(any(word in r["text"] for word in group) for group in item["keep"])
            dropped += not any(word in r["text"] for group in item["drop"] for word in group)
        print(json.dumps({
            "service": service, "model": mine[0]["model"], "calls": len(mine),
            "failed": sum(not r["ok"] for r in mine),
            "median_s": round(statistics.median(times), 2), "p90_s": times[int(len(times) * 0.9) - 1],
            "max_s": times[-1],
            "astro_terms_transcript": round(before / total, 3), "astro_terms_draft": round(after / total, 3),
            "talk_keep_ok": f"{kept}/{len(talk)}", "talk_drop_ok": f"{dropped}/{len(talk)}",
        }, ensure_ascii=False), flush=True)


if __name__ == "__main__":
    main()
