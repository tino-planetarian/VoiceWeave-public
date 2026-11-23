"""Fast path: encode only the recorded length instead of Whisper's fixed 30 s window.

On the GTX 1660 Ti the 30 s encoder pass is ~1.0 s of a ~1.2 s recognition, while
a 10 s window takes ~0.17 s. A shorter window can, however, change words or loop.
Every result is therefore checked; on any doubt this returns None and the caller
runs the normal full-window transcription. Nothing here stores audio or text.
"""
from __future__ import annotations

import zlib

SAMPLE_RATE = 16_000
FRAMES_PER_SECOND = 100
# Whisper's own hallucination limits (faster-whisper defaults).
COMPRESSION_RATIO_LIMIT = 2.4
AVG_LOGPROB_LIMIT = -1.0
# Japanese speech stays well below this many tokens per second of audio.
TOKENS_PER_SECOND_LIMIT = 25
# Above this, the window is probably silence; the full path's VAD decides instead.
NO_SPEECH_LIMIT = 0.6
# A stretch this long immediately repeated is treated as a decoding loop, not speech.
REPEAT_MIN_CHARS = 8
# More characters per second of speech than anyone speaks: the decoder repeated or
# invented text (2026-10-06: two kotoba-whisper outputs at 15-16, every correct output
# of 180 synthetic recordings, including very fast speech, at 11.7 or less).
DENSE_CHARS_PER_SECOND = 14.0


def _compression_ratio(text: str) -> float:
    data = text.encode("utf-8")
    return len(data) / max(1, len(zlib.compress(data)))


def spoken_text_tokens(tokens: list[int], timestamp_begin: int, speech_seconds: float,
                       seconds_per_step: float = 0.02) -> list[int]:
    """Text tokens of the segments that start while there is speech.

    In timestamp mode each segment opens with a timestamp token. The window is padded
    with silence up to 15 s, and Whisper sometimes "fills" that silence with a stock
    closing line ("ご視聴ありがとうございました"). A segment that starts at or after the
    end of the speech cannot hold spoken words, so its text is dropped.
    """
    kept: list[int] = []
    segment: list[int] = []
    start: float | None = None
    for token in tokens:
        if token >= timestamp_begin:
            time = (token - timestamp_begin) * seconds_per_step
            if start is None:
                start = time
            else:
                if start < speech_seconds:
                    kept += segment
                segment, start = [], None
        else:
            segment.append(token)
    if segment and (start is None or start < speech_seconds):
        kept += segment
    return kept


def too_dense(text: str, speech_seconds: float, limit: float = DENSE_CHARS_PER_SECOND) -> bool:
    """True when the text is longer than the speech could hold (at least 1 s of speech
    is assumed, so a short word after a short sound is never flagged)."""
    chars = sum(1 for c in text if not c.isspace() and c not in "、。，．!?！？")
    return chars > limit * max(speech_seconds, 1.0)


def has_immediate_repeat(text: str, min_chars: int = REPEAT_MIN_CHARS) -> bool:
    """True when some stretch of at least min_chars characters is directly repeated.

    A shortened window can make the decoder emit the whole utterance twice; ordinary
    speech practically never repeats eight or more characters back to back.
    """
    chars = [c for c in text if not c.isspace() and c not in "、。，．!?！？"]
    n = len(chars)
    for length in range(min_chars, n // 2 + 1):
        for start in range(0, n - 2 * length + 1):
            if chars[start:start + length] == chars[start + length:start + 2 * length]:
                return True
    return False


def _speech_only(audio):
    """Concatenated speech regions, or None when the detector finds no speech."""
    import numpy as np
    from faster_whisper.vad import VadOptions, collect_chunks, get_speech_timestamps

    regions = get_speech_timestamps(audio, VadOptions())
    if not regions:
        return None
    chunks, _ = collect_chunks(audio, regions)
    speech = np.concatenate(chunks, axis=0)
    return speech if len(speech) else None


def transcribe_short_window(model, audio, *, beam_size: int, initial_prompt: str | None,
                            pad_frames: int = 100, min_frames: int = 1500,
                            max_audio_seconds: float = 25.0):
    """Return (text, details), or (None, {"reject": code}) to request the full path.

    The reject code is one fixed word so diagnostics can say why, without any text.
    """
    import time

    import ctranslate2
    import numpy as np
    from faster_whisper.tokenizer import Tokenizer

    started = time.perf_counter()
    if len(audio) / SAMPLE_RATE > max_audio_seconds:
        return None, {"reject": "too_long"}
    # Same voice-activity step as the full path (default Silero options). Without it a
    # silent or noise-only recording decodes into invented text; with it, only speech
    # reaches the encoder. No speech at all is left to the full path, which yields "".
    speech = _speech_only(audio)
    if speech is None:
        return None, {"reject": "no_speech"}
    audio = speech
    seconds = len(audio) / SAMPLE_RATE
    features = model.feature_extractor(audio)
    natural = features.shape[1]
    frames = min(3000, max(min_frames, natural + pad_frames))
    frames += frames % 2  # the encoder halves the frame count
    if features.shape[1] < frames:
        features = np.pad(features, ((0, 0), (0, frames - features.shape[1])))
    features = np.ascontiguousarray(features[None, :, :frames], dtype=np.float32)

    tokenizer = Tokenizer(model.hf_tokenizer, model.model.is_multilingual,
                          task="transcribe", language="ja")
    prompt: list[int] = []
    if initial_prompt:
        prompt = [tokenizer.sot_prev] + tokenizer.encode(" " + initial_prompt.strip())[-223:]
    # Timestamp mode (no <|notimestamps|>): it ends segments cleanly and, in tests,
    # avoided the repetition loops seen without timestamps.
    prompt += list(tokenizer.sot_sequence)
    prepared = time.perf_counter()
    budget = int(seconds * TOKENS_PER_SECOND_LIMIT) + 20
    max_length = min(448, len(prompt) + budget)
    result = model.model.generate(
        ctranslate2.StorageView.from_array(features), [prompt], beam_size=beam_size,
        max_length=max_length, return_scores=True, return_no_speech_prob=True,
        suppress_blank=True, suppress_tokens=[-1],
    )[0]
    generated = time.perf_counter()
    tokens = result.sequences_ids[0]
    text_tokens = spoken_text_tokens(tokens, tokenizer.timestamp_begin, seconds)
    text = tokenizer.decode(text_tokens).strip()
    ratio = _compression_ratio(text)
    score = result.scores[0]
    reject = (
        "empty" if not text
        else "token_limit" if len(prompt) + len(tokens) >= max_length
        else "compression" if ratio > COMPRESSION_RATIO_LIMIT
        else "low_confidence" if score < AVG_LOGPROB_LIMIT
        else "no_speech" if result.no_speech_prob > NO_SPEECH_LIMIT
        else "repeat" if has_immediate_repeat(text)
        else "too_dense" if too_dense(text, seconds)
        else None
    )
    if reject is not None:
        return None, {"reject": reject}
    return text, {
        "window_frames": frames,
        "prepare_ms": round((prepared - started) * 1000),
        "generate_ms": round((generated - prepared) * 1000),
    }
