"""Compare recognition settings on the evaluation sentences: accuracy, punctuation, speed.

Usage:
  python scripts/eval/eval-asr.py --audio-dir DIR --model turbo=DIR [--model kotoba=DIR]
      [--cuda-dir DIR] [--beam 1,5] [--prompt none,punct] [--compute int8_float16]
      [--punctuator PATH] [--out FILE.json]

Audio: scripts/eval/make-asr-audio.ps1 (synthetic) or the user's own recordings named
<condition>_<id>.wav, kept outside the repository. Each file goes through the same path
as the app (fast short window, then the full 30 s window when a guard rejects it).

Metrics, all on the reference sentences in asr_eval_ja.tsv:
- CER: character error rate after removing punctuation and spaces (NFKC), with numbers
  in digits and equally correct spellings settled (三つ = 3つ, 分かる = わかる).
- Punctuation: precision/recall/F1 of 、/。 positions, aligned on the characters both
  texts share; "same mark" also requires the right kind (、 vs 。).
- Time: recognition time per file (the app adds about 0.1 s around it).
"--punctuator" runs the app's own rule-based punctuation (Rust, example
punctuate_stdin) on the recognized text, so "app" rows match what the app would type.
The JSON output holds aggregates and per-file texts; keep it outside the repository when
the audio is the user's voice.
"""
from __future__ import annotations

import argparse
import difflib
import json
import os
import statistics
import subprocess
import sys
import time
import unicodedata
import wave
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "worker"))

MARKS = {"、": "、", "，": "、", ",": "、", "。": "。", "．": "。", "？": "。", "?": "。", "！": "。", "!": "。"}
IGNORED = set("・「」『』（）() 　")
# A punctuated sentence of ordinary prose: shows Whisper the expected style without
# naming any word from the evaluation sentences.
PUNCT_PROMPT = "こんにちは。今日は、音声入力の確認をしています。よろしくお願いします。"


KANJI_DIGITS = {"〇": 0, "一": 1, "二": 2, "三": 3, "四": 4, "五": 5, "六": 6, "七": 7, "八": 8, "九": 9}
KANJI_UNITS = {"十": 10, "百": 100, "千": 1000}
# Spellings that are equally correct; not counted as recognition errors.
VARIANTS = {"分か": "わか", "下さい": "ください", "出来": "でき", "事": "こと"}


def kanji_number(run: str) -> str:
    """Kanji numerals ("五十万", "十二", "三") as digits ("50万", "12", "3")."""
    total, section, digit = 0, 0, 0
    for char in run:
        if char in KANJI_DIGITS:
            digit = KANJI_DIGITS[char]
        elif char in KANJI_UNITS:
            section += (digit or 1) * KANJI_UNITS[char]
            digit = 0
        elif char == "万":
            total += (section + digit) * 10000
            section, digit = 0, 0
    value = total + section + digit
    return f"{value // 10000}万" if total and value % 10000 == 0 else str(value)


def normalize_style(text: str) -> str:
    """Write numbers in digits and settle equally correct spellings, so the error rate
    counts recognition mistakes rather than style. Used for the error rate only."""
    import re

    for variant, canonical in VARIANTS.items():
        text = text.replace(variant, canonical)
    # A kanji number followed by a counter or unit (三つ, 二点, 十一月, 五十万円).
    return re.sub(r"[〇一二三四五六七八九十百千万]+(?=[つ点件月日時分年人万円回個本枚])",
                  lambda match: kanji_number(match.group(0)), text)


def strip_with_marks(text: str) -> tuple[str, set[tuple[int, str]]]:
    """Characters without punctuation, and (position, mark) for each 、/。."""
    text = unicodedata.normalize("NFKC", text)
    chars: list[str] = []
    marks: set[tuple[int, str]] = set()
    for char in text:
        # NFKC turns 、。 into themselves but ，． into ASCII; map them back via MARKS.
        if char in MARKS:
            if chars:
                marks.add((len(chars), MARKS[char]))
        elif char not in IGNORED and not char.isspace():
            chars.append(char)
    # A final 。 is nearly always present or added; count only marks inside the text.
    marks = {mark for mark in marks if mark[0] < len(chars)}
    return "".join(chars), marks


def edit_distance(a: str, b: str) -> int:
    row = list(range(len(b) + 1))
    for i, ca in enumerate(a, 1):
        prev, row[0] = row[0], i
        for j, cb in enumerate(b, 1):
            prev, row[j] = row[j], min(row[j] + 1, row[j - 1] + 1, prev + (ca != cb))
    return row[-1]


def punctuation_counts(reference: str, hypothesis: str) -> dict[str, int]:
    ref_chars, ref_marks = strip_with_marks(reference)
    hyp_chars, hyp_marks = strip_with_marks(hypothesis)
    mapping: dict[int, int] = {}
    for block in difflib.SequenceMatcher(None, hyp_chars, ref_chars, autojunk=False).get_matching_blocks():
        for offset in range(block.size + 1):
            mapping.setdefault(block.a + offset, block.b + offset)
    mapped = {(mapping[position], mark) for position, mark in hyp_marks if position in mapping}
    ref_positions = {position for position, _ in ref_marks}
    return {
        "ref": len(ref_marks), "hyp": len(hyp_marks),
        "hit": sum(1 for position, _ in mapped if position in ref_positions),
        "same_mark": len(mapped & ref_marks),
    }


def f1(hit: int, hyp: int, ref: int) -> dict[str, float]:
    precision = hit / hyp if hyp else 0.0
    recall = hit / ref if ref else 0.0
    score = 2 * precision * recall / (precision + recall) if precision + recall else 0.0
    return {"precision": round(precision, 3), "recall": round(recall, 3), "f1": round(score, 3)}


def load(path: Path) -> np.ndarray:
    with wave.open(str(path)) as w:
        assert w.getframerate() == 16000 and w.getnchannels() == 1 and w.getsampwidth() == 2
        return (np.frombuffer(w.readframes(w.getnframes()), "<i2").astype(np.float32) / 32768).copy()


def recognize(model, audio, beam: int, prompt: str | None) -> tuple[str, str, float]:
    from voiceweave_worker.short_window import transcribe_short_window
    from voiceweave_worker.timing import transcribe_measured

    started = time.perf_counter()
    try:
        text, _ = transcribe_short_window(model, audio, beam_size=beam, initial_prompt=prompt)
    except Exception:
        text = None
    path = "short"
    if text is None:
        text, _, _ = transcribe_measured(model, audio, beam_size=beam, initial_prompt=prompt)
        path = "full"
    return text, path, time.perf_counter() - started


def app_punctuation(punctuator: str | None, texts: list[str]) -> list[str]:
    if not punctuator:
        return texts
    joined = "\n".join(text.replace("\n", " ") for text in texts) + "\n"
    result = subprocess.run([str(Path(punctuator).resolve())], input=joined.encode("utf-8"), capture_output=True, check=True)
    lines = result.stdout.decode("utf-8").split("\n")
    return lines[: len(texts)]


def summarize(rows: list[dict], key: str) -> dict:
    errors = sum(row[f"{key}_errors"] for row in rows)
    length = sum(row["ref_len"] for row in rows)
    counts = {name: sum(row[f"{key}_punct"][name] for row in rows) for name in ("ref", "hyp", "hit", "same_mark")}
    return {
        "cer": round(errors / length, 4),
        "punct_position": f1(counts["hit"], counts["hyp"], counts["ref"]),
        "punct_same_mark": f1(counts["same_mark"], counts["hyp"], counts["ref"]),
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--audio-dir", type=Path, required=True)
    parser.add_argument("--model", action="append", required=True, help="name=directory")
    parser.add_argument("--cuda-dir")
    parser.add_argument("--compute", default="int8_float16")
    parser.add_argument("--beam", default="1")
    parser.add_argument("--prompt", default="none")
    parser.add_argument("--conditions", default="normal,fast,nopause")
    parser.add_argument("--punctuator")
    parser.add_argument("--out", type=Path)
    args = parser.parse_args()
    if args.cuda_dir:
        os.add_dll_directory(args.cuda_dir)
        os.environ["PATH"] = args.cuda_dir + os.pathsep + os.environ["PATH"]
    from faster_whisper import WhisperModel

    references = {}
    for line in (Path(__file__).with_name("asr_eval_ja.tsv")).read_text(encoding="utf-8").splitlines():
        if line.strip():
            sentence_id, text = line.split("\t", 1)
            references[sentence_id] = text
    conditions = args.conditions.split(",")
    files = [(condition, sentence_id, args.audio_dir / f"{condition}_{sentence_id}.wav")
             for condition in conditions for sentence_id in references]
    files = [entry for entry in files if entry[2].exists()]
    audio = {entry[2]: load(entry[2]) for entry in files}
    results = []
    for spec in args.model:
        name, directory = spec.split("=", 1)
        loaded = time.perf_counter()
        model = WhisperModel(directory, device="cuda" if args.cuda_dir else "cpu",
                             compute_type=args.compute if args.cuda_dir else "int8", local_files_only=True)
        load_s = time.perf_counter() - loaded
        recognize(model, audio[files[0][2]], 1, None)  # warm-up, not timed
        for beam in [int(value) for value in args.beam.split(",")]:
            for prompt_name in args.prompt.split(","):
                prompt = PUNCT_PROMPT if prompt_name == "punct" else None
                rows = []
                for condition, sentence_id, path in files:
                    text, route, seconds = recognize(model, audio[path], beam, prompt)
                    rows.append({"condition": condition, "id": sentence_id, "raw": text,
                                 "route": route, "seconds": round(seconds, 3)})
                finals = app_punctuation(args.punctuator, [row["raw"] for row in rows])
                for row, final in zip(rows, finals):
                    reference = references[row["id"]]
                    ref_chars, _ = strip_with_marks(reference)
                    row["app"] = final
                    row["ref_len"] = len(normalize_style(ref_chars))
                    for key in ("raw", "app"):
                        row[f"{key}_errors"] = edit_distance(
                            normalize_style(strip_with_marks(row[key])[0]), normalize_style(ref_chars))
                        row[f"{key}_punct"] = punctuation_counts(reference, row[key])
                config = {"model": name, "beam": beam, "prompt": prompt_name, "compute": args.compute,
                          "load_s": round(load_s, 1)}
                per_condition = {}
                for condition in conditions:
                    subset = [row for row in rows if row["condition"] == condition]
                    if not subset:
                        continue
                    times = sorted(row["seconds"] for row in subset)
                    per_condition[condition] = {
                        "whisper": summarize(subset, "raw"), "app": summarize(subset, "app"),
                        "median_s": round(statistics.median(times), 3), "max_s": times[-1],
                        "full_path": sum(row["route"] == "full" for row in subset),
                    }
                summary = {**config, "all": {"whisper": summarize(rows, "raw"), "app": summarize(rows, "app")},
                           "conditions": per_condition}
                print(json.dumps(summary, ensure_ascii=False), flush=True)
                results.append({**summary, "rows": rows})
        del model
    if args.out:
        args.out.write_text(json.dumps(results, ensure_ascii=False, indent=1), encoding="utf-8")


if __name__ == "__main__":
    main()
