from pathlib import Path

import numpy as np

from voiceweave_worker import sherpa_engine


def test_files_present_needs_every_file(tmp_path: Path):
    assert not sherpa_engine.files_present("nemo_ctc", tmp_path)
    (tmp_path / "model.int8.onnx").write_bytes(b"x")
    assert not sherpa_engine.files_present("nemo_ctc", tmp_path)
    (tmp_path / "tokens.txt").write_text("x")
    assert sherpa_engine.files_present("nemo_ctc", tmp_path)
    assert not sherpa_engine.files_present("whisper", tmp_path)


def test_english_transducer_needs_all_three_parts(tmp_path: Path):
    for part in ("encoder", "decoder"):
        (tmp_path / f"{part}.int8.onnx").write_bytes(b"x")
    (tmp_path / "tokens.txt").write_text("x")
    assert not sherpa_engine.files_present("nemo_transducer", tmp_path)
    (tmp_path / "joiner.int8.onnx").write_bytes(b"x")
    assert sherpa_engine.files_present("nemo_transducer", tmp_path)
    assert sherpa_engine.files("nemo_transducer", tmp_path)["encoder"].name == "encoder.int8.onnx"


def test_sounds_that_are_not_speech_are_not_written_as_words():
    class Fake(sherpa_engine.SherpaModel):
        def __init__(self):
            self.english = False

        def _decode(self, audio):
            raise AssertionError("silence must not reach the recognizer")

    # Silence holds no speech: nothing is recognized, nothing is written.
    assert sherpa_engine.speech_span(np.zeros(16_000 * 2, dtype=np.float32)) is None
    assert Fake().recognize(np.zeros(16_000 * 2, dtype=np.float32)) == ""


def test_a_part_ends_by_what_its_words_say_not_by_the_pause():
    close = sherpa_engine.close_part
    # A phrase still open: no mark, whatever the pause.
    for text in ("この機能の", "設定画面を", "きのうの会議で決まったことを", "画面の右上にあるボタンを", "来週のスケジュールは"):
        assert close(text) == text
    # A clause that goes on with the next words: 、
    for text in ("まずログインの処理を確認して", "明日の打ち合わせなんですけど", "時間がかかるので", "このボタンを押すと", "読んで"):
        assert close(text) == text + "、"
    # A sentence has ended: 。
    for text in ("これでいいと思います", "新しい画面の案を作りました", "ログの出力先を変えたいです", "確認をお願いします", "今日はやらない"):
        assert close(text) == text + "。"
    # The conjunction ですが is not the particle が.
    assert close("お願いしたいんですが") == "お願いしたいんですが、"


def test_the_engine_mark_at_the_end_of_a_part_is_dropped_but_the_final_one_is_kept(monkeypatch):
    # The parts are taken as speech whole (the voice detector has its own test).
    monkeypatch.setattr(sherpa_engine, "speech_span", lambda part: (0, len(part)))
    class Fake(sherpa_engine.SherpaModel):
        def __init__(self, texts):
            self.english = False
            self._texts = iter(texts)

        def _decode(self, audio):
            return next(self._texts)

    audio = np.zeros(16_000 * 40, dtype=np.float32)  # two parts of at most 25 s
    # In the middle of a recording the engine's 。 after a phrase still open is removed.
    model = Fake(["この機能の", "設定画面を。"])
    assert model.recognize(audio, final=False) == "この機能の設定画面を"
    model = Fake(["この機能の。", "設定画面を直したいです。"])
    assert model.recognize(audio) == "この機能の設定画面を直したいです。"


def test_parts_end_with_a_mark():
    assert sherpa_engine.close_part("明日は晴れです") == "明日は晴れです。"
    assert sherpa_engine.close_part("それで明日の件ですが") == "それで明日の件ですが、"
    assert sherpa_engine.close_part("聞こえてますか?") == "聞こえてますか？"
    assert sherpa_engine.close_part("終わりです。") == "終わりです。"
    assert sherpa_engine.close_part("GitHub") == "GitHub"
    assert sherpa_engine.close_part("") == ""


def test_long_audio_is_cut_into_parts_within_the_limit():
    audio = np.zeros(16_000 * 60, dtype=np.float32)
    parts = sherpa_engine.split_long(audio)
    assert sum(len(part) for part in parts) == len(audio)
    assert all(len(part) <= 16_000 * sherpa_engine.MAX_PART_SECONDS for part in parts)
    assert len(sherpa_engine.split_long(audio[:16_000 * 10])) == 1


def test_loading_refuses_unknown_engines_and_missing_files(tmp_path: Path):
    for engine in ("whisper", "nemo_ctc"):
        try:
            sherpa_engine.load(engine, tmp_path)
        except (ValueError, FileNotFoundError):
            pass
        else:
            raise AssertionError(engine)
