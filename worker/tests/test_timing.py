from types import SimpleNamespace

import pytest

from voiceweave_worker import timing


def test_lazy_segments_are_included_and_settings_are_preserved(monkeypatch, capsys):
    ticks = iter([1.0, 1.2, 10.0])
    monkeypatch.setattr(timing.time, "perf_counter", lambda: next(ticks))
    consumed = []
    class Model:
        def transcribe(self, audio, **kwargs):
            assert audio is sentinel
            assert kwargs == dict(language="ja", task="transcribe", beam_size=1,
                temperature=0, condition_on_previous_text=False, vad_filter=True,
                word_timestamps=False, initial_prompt="synthetic hint")
            def segments():
                consumed.append(True)
                yield SimpleNamespace(text="synthetic recognition")
            return segments(), SimpleNamespace(language="ja")
    sentinel = object()
    text, info, durations = timing.transcribe_measured(Model(), sentinel,
        beam_size=1, initial_prompt="synthetic hint")
    assert consumed == [True]
    assert text == "synthetic recognition" and info.language == "ja"
    assert durations == {"elapsed_ms": 9000, "frontend_ms": 200, "decode_ms": 8800}
    assert capsys.readouterr() == ("", "")


def test_lazy_failure_is_not_converted_to_a_success_or_logged(monkeypatch, capsys):
    monkeypatch.setattr(timing.time, "perf_counter", lambda: 1.0)
    class Model:
        def transcribe(self, audio, **kwargs):
            def segments():
                yield SimpleNamespace(text="synthetic private fragment")
                raise RuntimeError("synthetic decoding failure")
            return segments(), None
    with pytest.raises(RuntimeError):
        timing.transcribe_measured(Model(), object(), beam_size=1, initial_prompt=None)
    assert capsys.readouterr() == ("", "")


def test_empty_segments_are_successful_with_measured_generation(monkeypatch):
    ticks = iter([2.0, 2.25, 2.3])
    monkeypatch.setattr(timing.time, "perf_counter", lambda: next(ticks))
    class Model:
        def transcribe(self, audio, **kwargs):
            return iter(()), SimpleNamespace(language="ja")
    text, _, durations = timing.transcribe_measured(Model(), object(), beam_size=1, initial_prompt=None)
    assert text == ""
    assert durations == {"elapsed_ms": 300, "frontend_ms": 250, "decode_ms": 50}
