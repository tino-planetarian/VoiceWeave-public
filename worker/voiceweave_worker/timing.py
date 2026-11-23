"""Measure eager audio preparation and lazy decoding without retaining speech."""
from __future__ import annotations

import time


def transcribe_measured(model, audio, *, beam_size: int, initial_prompt: str | None):
    started = time.perf_counter()
    segments, info = model.transcribe(
        audio,
        language="ja",
        task="transcribe",
        beam_size=beam_size,
        temperature=0,
        condition_on_previous_text=False,
        vad_filter=True,
        word_timestamps=False,
        initial_prompt=initial_prompt,
    )
    prepared = time.perf_counter()
    # faster-whisper defers generation until segments are consumed.
    raw_text = "".join(segment.text for segment in segments)
    finished = time.perf_counter()
    return raw_text, info, {
        "elapsed_ms": round((finished - started) * 1000),
        "frontend_ms": round((prepared - started) * 1000),
        "decode_ms": round((finished - prepared) * 1000),
    }
