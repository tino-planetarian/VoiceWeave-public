"""How often field terms come out right, with and without recognition hints.

Usage:
  python scripts/eval/eval-terms.py --audio-dir DIR --model standard=DIR [--cuda-dir DIR]
      [--conditions normal,fast] [--hints none,screen,unrelated,field] [--align PATH]
      [--out FILE.json]

Audio: scripts/eval/make-asr-audio.ps1 -Tsv astro_eval_ja.tsv (synthetic voice). The terms
counted in each sentence are the glossary terms (src-tauri/src/glossary.rs) written in it,
longest first, so ペルセウス座流星群 counts once and not also as 流星群. A term is right when
it appears in the recognized text exactly (after NFKC, without spaces and punctuation).

Hints, given to Whisper the way the app gives them (the initial prompt):
- none: no hint.
- screen: the sentence's own glossary terms plus other glossary terms, 8 in all, joined
  with 、 (the F8 case where the topic's words are on the screen).
- unrelated: 8 glossary terms that are not in the sentence (astronomy words on the screen,
  but not these ones): shows whether hints that do not match cause harm.
- field: one natural sentence naming the field, with no term list.
- topicN: two passes. The first transcript (no hint) names topics: every glossary line
  with a term found in it; the second pass gets those lines' terms (N characters at most,
  default 200, the app's hint limit). Works with no screen; time is both passes.
"--align" runs the app's glossary spelling step (Rust, example align_stdin) on the
recognized text; "aligned_accuracy" is the term accuracy after it (what the app types).
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import os
import random
import re
import statistics
import subprocess
import sys
import unicodedata
from pathlib import Path

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("eval_asr", HERE / "eval-asr.py")
eval_asr = importlib.util.module_from_spec(spec)
spec.loader.exec_module(eval_asr)

FIELD_PROMPT = "プラネタリウムの星空解説です。惑星や星座、流星群、天体の見え方について話します。"
HINT_TERMS = 8
# A one-character term (衝, 留, 合) counts only when a particle follows it, so 合 inside
# 組み合わせ is not taken for the astronomical term.
PARTICLES = "をがはのとにで"


def glossary_terms() -> list[str]:
    source = (HERE.parents[1] / "src-tauri" / "src" / "glossary.rs").read_text(encoding="utf-8")
    body = source.split('DEFAULT_TEXT: &str = "\\', 1)[1].split('";', 1)[0]
    out: list[str] = []
    for term in re.split(r"[\n\r、,，　 ]", body):
        term = term.strip()
        if term and term not in out:
            out.append(term)
    return out


def glossary_lines() -> list[list[str]]:
    """The default glossary's lines (each line is one topic: phenomena, coordinates, ...)."""
    source = (HERE.parents[1] / "src-tauri" / "src" / "glossary.rs").read_text(encoding="utf-8")
    body = source.split('DEFAULT_TEXT: &str = "\\', 1)[1].split('";', 1)[0]
    lines = []
    for line in body.splitlines():
        terms = [term.strip() for term in re.split(r"[、,，　 ]", line) if term.strip()]
        if terms:
            lines.append(terms)
    return lines


def topic_hint(first: str, lines: list[list[str]], glossary: list[str], limit: int) -> str | None:
    """Hints for a second pass: the terms of every glossary line (topic) that has a term in
    the first transcript, lines with more such terms first, within `limit` characters."""
    found = expected_terms(first, glossary)
    scored = sorted(((sum(term in line for term in found), index) for index, line in enumerate(lines)),
                    reverse=True)
    out: list[str] = []
    used = 0
    for score, index in scored:
        if score == 0:
            break
        for term in lines[index]:
            if term in out or used + len(term) + 1 > limit:
                continue
            out.append(term)
            used += len(term) + 1
    return "、".join(out) or None


def clean(text: str) -> str:
    text = unicodedata.normalize("NFKC", text)
    return "".join(ch for ch in text if ch not in eval_asr.IGNORED and ch not in eval_asr.MARKS)


def expected_terms(reference: str, glossary: list[str]) -> list[str]:
    masked = clean(reference)
    found = []
    for term in sorted(glossary, key=len, reverse=True):
        key = clean(term)
        start = 0
        while (index := masked.find(key, start)) >= 0:
            after = masked[index + len(key): index + len(key) + 1]
            if len(key) > 1 or (after and after in PARTICLES):
                found.append(term)
                masked = masked[:index] + "\0" * len(key) + masked[index + len(key):]
                break
            start = index + 1
    return found


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--audio-dir", type=Path, required=True)
    parser.add_argument("--model", action="append", required=True, help="name=directory")
    parser.add_argument("--cuda-dir")
    parser.add_argument("--compute", default="int8_float16")
    parser.add_argument("--beam", type=int, default=1)
    parser.add_argument("--conditions", default="normal,fast")
    parser.add_argument("--hints", default="none,screen,unrelated,field")
    parser.add_argument("--align")
    parser.add_argument("--out", type=Path)
    args = parser.parse_args()
    if args.cuda_dir:
        os.add_dll_directory(args.cuda_dir)
        os.environ["PATH"] = args.cuda_dir + os.pathsep + os.environ["PATH"]
    from faster_whisper import WhisperModel

    glossary = glossary_terms()
    lines = glossary_lines()
    references = {}
    for line in (HERE / "astro_eval_ja.tsv").read_text(encoding="utf-8").splitlines():
        if line.strip():
            parts = line.split("\t")
            references[parts[0]] = parts[1]
    expected = {key: expected_terms(text, glossary) for key, text in references.items()}
    rng = random.Random(20261012)
    hints = {}
    for key in references:
        own = expected[key]
        others = [term for term in glossary if term not in own and term not in references[key]]
        fill = rng.sample(others, HINT_TERMS)
        hints[key] = {
            "none": None,
            "screen": "、".join((own + fill)[:HINT_TERMS] if len(own) < HINT_TERMS else own),
            "unrelated": "、".join(fill),
            "field": FIELD_PROMPT,
        }
    files = [(condition, key, args.audio_dir / f"{condition}_{key}.wav")
             for condition in args.conditions.split(",") for key in references]
    files = [entry for entry in files if entry[2].exists()]
    audio = {entry[2]: eval_asr.load(entry[2]) for entry in files}
    results = []
    for spec_text in args.model:
        name, directory = spec_text.split("=", 1)
        model = WhisperModel(directory, device="cuda" if args.cuda_dir else "cpu",
                             compute_type=args.compute if args.cuda_dir else "int8", local_files_only=True)
        eval_asr.recognize(model, audio[files[0][2]], 1, None)  # warm-up, not timed
        for hint_name in args.hints.split(","):
            rows = []
            for condition, key, path in files:
                if hint_name.startswith("topic"):
                    # Two passes: the first transcript picks the topics, the second uses them.
                    first, _, first_s = eval_asr.recognize(model, audio[path], args.beam, None)
                    prompt = topic_hint(first, lines, glossary, int(hint_name[5:] or 200))
                    text, route, seconds = eval_asr.recognize(model, audio[path], args.beam, prompt)
                    seconds += first_s
                else:
                    prompt = hints[key][hint_name]
                    text, route, seconds = eval_asr.recognize(model, audio[path], args.beam, prompt)
                hypothesis = clean(text)
                hits = [term for term in expected[key] if clean(term) in hypothesis]
                reference = eval_asr.normalize_style(clean(references[key]))
                rows.append({
                    "condition": condition, "id": key, "raw": text, "route": route,
                    "seconds": round(seconds, 3), "terms": expected[key],
                    "missed": [term for term in expected[key] if term not in hits],
                    "errors": eval_asr.edit_distance(eval_asr.normalize_style(hypothesis), reference),
                    "ref_len": len(reference),
                })
            if args.align:
                joined = "\n".join(row["raw"].replace("\n", " ") for row in rows) + "\n"
                done = subprocess.run([str(Path(args.align).resolve())], input=joined.encode("utf-8"),
                                      capture_output=True, check=True)
                for row, line in zip(rows, done.stdout.decode("utf-8").split("\n")):
                    row["aligned"] = line
                    row["aligned_missed"] = [term for term in row["terms"] if clean(term) not in clean(line)]
            total = sum(len(row["terms"]) for row in rows)
            missed = sum(len(row["missed"]) for row in rows)
            summary = {
                "model": name, "hints": hint_name, "files": len(rows),
                "term_accuracy": round(1 - missed / total, 3), "terms": total, "missed": missed,
                **({"aligned_accuracy": round(1 - sum(len(row["aligned_missed"]) for row in rows) / total, 3)}
                   if args.align else {}),
                "cer": round(sum(row["errors"] for row in rows) / sum(row["ref_len"] for row in rows), 4),
                "median_s": round(statistics.median(row["seconds"] for row in rows), 3),
                "full_path": sum(row["route"] == "full" for row in rows),
            }
            print(json.dumps(summary, ensure_ascii=False), flush=True)
            results.append({**summary, "rows": rows})
        del model
    if args.out:
        args.out.write_text(json.dumps(results, ensure_ascii=False, indent=1), encoding="utf-8")


if __name__ == "__main__":
    sys.exit(main())
