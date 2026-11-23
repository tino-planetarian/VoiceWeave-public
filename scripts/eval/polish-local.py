"""Polish synthetic dictation into email prose with local models served by Ollama.

Usage: python scripts/eval/polish-local.py --model qwen3:4b [--model gemma3:4b] --out DIR

Uses only the synthetic sentences in polish_samples_ja.json (no personal speech).
Writes DIR/<model>.json with each output, timings and a keyword check:
"keep" groups must survive (any spelling in the group), "drop" groups are
retracted words that should not reappear. The output text goes to DIR only,
which must be outside the repository.
"""
from __future__ import annotations

import argparse
import json
import re
import pathlib
import time
import urllib.request

PROMPT = """あなたは音声入力の書き起こしを、ビジネスメールの本文として読みやすく整える編集者です。
次の規則を必ず守ってください。
- 事実・数字・日付・人名・社名は変えない。数字の書き方（漢数字か算用数字か）も元のままにする。
- 誰が何をするか（依頼する側・される側、送る側・受け取る側）を入れ替えない。
- 書かれていない情報を足さない。件名、宛名、「拝啓」「敬具」、署名、理由、推測の言葉（「概算」「約」など）は書かない。
- 「えーと」「あのー」などの言いよどみは消す。話し手が言い直した場合は、取り消した内容は書かず、最後に言い直した内容だけを残す。
- 話し言葉を丁寧なビジネス敬語に整え、句読点を付け、内容のまとまりごとに段落を分ける。
- 出力は整えた本文だけ。説明・前置き・引用符は書かない。"""


def chat(host: str, model: str, text: str) -> dict:
    body = {
        "model": model,
        # Qwen3 ignores Ollama's think=false in some builds; its own switch is "/no_think".
        "messages": [{"role": "system", "content": PROMPT},
                     {"role": "user", "content": f"{text} /no_think" if model.startswith("qwen3") else text}],
        "stream": False,
        "think": False,
        "keep_alive": "10m",
        "options": {"temperature": 0.2, "num_ctx": 2048},
    }
    request = urllib.request.Request(
        f"{host}/api/chat", data=json.dumps(body).encode(), headers={"Content-Type": "application/json"})
    started = time.perf_counter()
    with urllib.request.urlopen(request, timeout=600) as response:
        data = json.load(response)
    data["wall_s"] = time.perf_counter() - started
    return data


def check(output: str, sample: dict) -> dict:
    output = output.replace(" ", "").replace("　", "")
    missing = [group[0] for group in sample["keep"] if not any(word in output for word in group)]
    revived = [group[0] for group in sample["drop"] if any(word in output for word in group)]
    return {"missing": missing, "revived": revived}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", action="append", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--host", default="http://127.0.0.1:11434")
    args = parser.parse_args()
    samples = json.loads((pathlib.Path(__file__).parent / "polish_samples_ja.json").read_text(encoding="utf-8"))
    out = pathlib.Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    for model in args.model:
        # The first call includes loading the model from disk; report it separately.
        warm = chat(args.host, model, "テストです")
        rows = []
        for sample in samples:
            data = chat(args.host, model, sample["text"])
            text = re.sub(r"<think>.*?</think>", "", data["message"]["content"], flags=re.S).strip()
            rows.append({
                "id": sample["id"], "output": text, "wall_s": round(data["wall_s"], 2),
                "eval_tokens": data.get("eval_count"),
                "tokens_per_s": round(data["eval_count"] / (data["eval_duration"] / 1e9), 1)
                if data.get("eval_duration") else None,
                **check(text, sample),
            })
        walls = sorted(row["wall_s"] for row in rows)
        summary = {
            "model": model, "load_s": round(warm.get("load_duration", 0) / 1e9, 2),
            "first_call_s": round(warm["wall_s"], 2), "median_s": walls[len(walls) // 2], "max_s": walls[-1],
            "keyword_failures": sum(bool(row["missing"] or row["revived"]) for row in rows),
        }
        (out / f"{model.replace(':', '_')}.json").write_text(
            json.dumps({"summary": summary, "rows": rows}, ensure_ascii=False, indent=1), encoding="utf-8")
        print(json.dumps(summary, ensure_ascii=False))


if __name__ == "__main__":
    main()
