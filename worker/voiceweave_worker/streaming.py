"""Recognition while the user is still speaking (design: docs/STREAMING_DESIGN.md).

The app sends the audio recorded since the last cut. If it holds a finished pause after
enough speech, the part up to the middle of that pause is recognized now, so that at
stop only the remainder is left. Nothing here is delivered on its own: the app joins
the parts and, on any doubt, recognizes the whole recording the usual way instead.
"""
from __future__ import annotations

SAMPLE_RATE = 16_000
# A breath between phrases is about 0.2-0.5 s.
MIN_GAP_SECONDS = 0.25
# Shorter parts gave the same accuracy in the 2026-10-06 evaluation (3 s and 6 s).
MIN_CHUNK_SECONDS = 3.0
# A pause counts only once this much of it has been heard (it may still be a word gap).
NOTICE_SECONDS = 0.3


def find_cut(audio, *, min_chunk: float = MIN_CHUNK_SECONDS, min_gap: float = MIN_GAP_SECONDS,
             notice: float = NOTICE_SECONDS) -> int | None:
    """Sample index of the middle of the last finished pause that leaves at least
    `min_chunk` seconds before it, or None."""
    from faster_whisper.vad import VadOptions, get_speech_timestamps

    if len(audio) < int((min_chunk + notice) * SAMPLE_RATE):
        return None
    speech = get_speech_timestamps(
        audio, VadOptions(min_silence_duration_ms=int(min_gap * 1000), speech_pad_ms=50))
    limit = len(audio) - int(notice * SAMPLE_RATE)
    cut = None
    for before, after in zip(speech, speech[1:]):
        gap_start, gap_end = before["end"], after["start"]
        middle = (gap_start + gap_end) // 2
        if (gap_end - gap_start) >= int(min_gap * SAMPLE_RATE) and middle >= int(min_chunk * SAMPLE_RATE) \
                and gap_end <= limit:
            cut = middle
    # Trailing silence after the last speech: also a finished pause.
    if speech and len(audio) - speech[-1]["end"] >= int((min_gap + notice) * SAMPLE_RATE):
        middle = speech[-1]["end"] + int(min_gap * SAMPLE_RATE) // 2
        if middle >= int(min_chunk * SAMPLE_RATE):
            cut = middle
    return cut


def stream_step(model, audio, *, prompt: str | None):
    """Return (cut, text, reject). cut None: nothing to do yet. reject: a fixed code when
    the part could not be trusted (the app then falls back to the whole recording)."""
    from voiceweave_worker.short_window import transcribe_short_window

    cut = find_cut(audio)
    if cut is None:
        return None, "", None
    part = audio[:cut]
    if hasattr(model, "recognize"):
        # Parakeet (sherpa_engine): no fast window or confidence checks.
        try:
            return cut, model.recognize(part, final=False).strip(), None
        except Exception:
            return cut, "", "exception"
    try:
        text, details = transcribe_short_window(model, part, beam_size=1, initial_prompt=prompt)
    except Exception:
        return cut, "", "exception"
    if text is None:
        reject = details.get("reject", "exception")
        # A part with no speech (only breath or noise) is simply empty.
        if reject == "no_speech":
            return cut, "", None
        return cut, "", reject
    return cut, text.strip(), None
