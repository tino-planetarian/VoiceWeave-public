"""Decisions of recognition while speaking, with the recognizer replaced by stand-ins."""
import numpy as np

from voiceweave_worker import short_window, streaming


def test_too_little_audio_never_reaches_the_detector(monkeypatch):
    def fail(*_args, **_kwargs):
        raise AssertionError("detector must not run")

    monkeypatch.setattr("faster_whisper.vad.get_speech_timestamps", fail)
    assert streaming.find_cut(np.zeros(16_000 * 3, dtype=np.float32)) is None


def test_the_last_finished_pause_after_enough_speech_is_the_cut(monkeypatch):
    rate = streaming.SAMPLE_RATE
    # Speech 0-2 s, pause, 2.4-5 s, pause, 5.5-7.8 s; the audio ends at 8 s.
    regions = [{"start": 0, "end": 2 * rate}, {"start": int(2.4 * rate), "end": 5 * rate},
               {"start": int(5.5 * rate), "end": int(7.8 * rate)}]
    monkeypatch.setattr("faster_whisper.vad.get_speech_timestamps", lambda *_a, **_k: regions)
    cut = streaming.find_cut(np.zeros(8 * rate, dtype=np.float32))
    # The 2.0-2.4 s pause is before 3 s of speech; 5.0-5.5 s is the last finished pause.
    assert cut == (5 * rate + int(5.5 * rate)) // 2


def test_a_pause_still_in_progress_is_not_used(monkeypatch):
    rate = streaming.SAMPLE_RATE
    # Speech until 4 s, and the audio ends 0.2 s into the following silence: too short to
    # tell a pause from a gap inside a word.
    regions = [{"start": 0, "end": 4 * rate}]
    monkeypatch.setattr("faster_whisper.vad.get_speech_timestamps", lambda *_a, **_k: regions)
    assert streaming.find_cut(np.zeros(int(4.2 * rate), dtype=np.float32)) is None
    # Once 0.55 s of silence has been heard, the pause is used.
    assert streaming.find_cut(np.zeros(int(4.6 * rate), dtype=np.float32)) == 4 * rate + int(0.25 * rate) // 2


def test_silent_parts_are_empty_and_untrusted_parts_are_rejected(monkeypatch):
    audio = np.zeros(16_000 * 5, dtype=np.float32)
    monkeypatch.setattr(streaming, "find_cut", lambda *_a, **_k: 16_000 * 4)
    monkeypatch.setattr(short_window, "transcribe_short_window", lambda *_a, **_k: (None, {"reject": "no_speech"}))
    assert streaming.stream_step(None, audio, prompt=None) == (16_000 * 4, "", None)
    monkeypatch.setattr(short_window, "transcribe_short_window", lambda *_a, **_k: (None, {"reject": "repeat"}))
    assert streaming.stream_step(None, audio, prompt=None) == (16_000 * 4, "", "repeat")
    monkeypatch.setattr(short_window, "transcribe_short_window", lambda *_a, **_k: (" 前半です。", {}))
    assert streaming.stream_step(None, audio, prompt="文脈") == (16_000 * 4, "前半です。", None)
    monkeypatch.setattr(streaming, "find_cut", lambda *_a, **_k: None)
    assert streaming.stream_step(None, audio, prompt=None) == (None, "", None)
