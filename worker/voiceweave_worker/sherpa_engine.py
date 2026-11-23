"""NVIDIA Parakeet, run with sherpa-onnx on the CPU (experimental): the Japanese model
(TDT-CTC 0.6B ja, exported as CTC) and the English one (TDT 0.6B v2, a transducer; used
by the English shortcut). Both write their own punctuation. No hints (neither takes
hotwords); the dictionary's replacements still apply in the app.

It takes the same 16 kHz float audio as Whisper and returns plain text. Long recordings
are cut at pauses into parts the model handles well, recognized in order and joined.
Importing this module loads nothing; `load` reads only the given local folder.
"""
from __future__ import annotations

import time
from pathlib import Path

import numpy as np

SAMPLE_RATE = 16_000
# Parts longer than this are cut at a pause (the model was trained on short utterances).
MAX_PART_SECONDS = 25.0

ENGINES = ("nemo_ctc", "nemo_transducer")


def speech_span(audio) -> tuple[int, int] | None:
    """(start, end) in samples of the speech in `audio`, or None when it holds none.

    These models must write something for any sound, so a key click, a breath or a beep on
    its own became words ("あっピッ", "ピッ", "フッ", 2026-10-30). The Whisper path has this
    voice detector (silero) built in; this one did not. Sounds shorter than 200 ms are not
    speech, and a little (100 ms) is kept around the speech."""
    from faster_whisper.vad import VadOptions, get_speech_timestamps

    regions = get_speech_timestamps(
        audio,
        VadOptions(threshold=0.5, min_speech_duration_ms=200, min_silence_duration_ms=2000,
                   speech_pad_ms=100),
    )
    if not regions:
        return None
    return regions[0]["start"], regions[-1]["end"]


def files(engine: str, folder: Path) -> dict[str, Path] | None:
    """The model files by role, or None when one is missing."""
    if engine not in ENGINES:
        return None
    # The int8 file when the folder holds both.
    parts = ("model",) if engine == "nemo_ctc" else ("encoder", "decoder", "joiner")
    found = {}
    for part in parts:
        models = sorted(folder.glob(f"{part}*.onnx"), key=lambda path: ".int8." not in path.name)
        found[part] = models[0] if models else None
    found["tokens"] = folder / "tokens.txt"
    if any(path is None or not path.is_file() for path in found.values()):
        return None
    return found


def files_present(engine: str, folder: Path) -> bool:
    return files(engine, folder) is not None


class SherpaModel:
    """One loaded recognizer. `recognize` handles any length."""

    def __init__(self, engine: str, folder: Path, threads: int):
        import sherpa_onnx

        found = files(engine, folder)
        self.english = engine == "nemo_transducer"
        # Milliseconds the last `recognize` spent on the voice detector (the rest is the
        # recognizer itself), for the diagnostics.
        self.prepare_ms = 0
        if self.english:
            self._recognizer = sherpa_onnx.OfflineRecognizer.from_transducer(
                encoder=str(found["encoder"]),
                decoder=str(found["decoder"]),
                joiner=str(found["joiner"]),
                tokens=str(found["tokens"]),
                num_threads=threads,
                model_type="nemo_transducer",
            )
        else:
            self._recognizer = sherpa_onnx.OfflineRecognizer.from_nemo_ctc(
                model=str(found["model"]),
                tokens=str(found["tokens"]),
                num_threads=threads,
            )

    def warm_up(self) -> None:
        """One real inference on silence, for the readiness check (`recognize` would skip
        silence)."""
        self._decode(np.zeros(SAMPLE_RATE, dtype=np.float32))

    def use_hints(self, hints: list[str]) -> None:
        """Hints before recognition: not taken by this model (see above)."""

    def _decode(self, audio) -> str:
        stream = self._recognizer.create_stream()
        stream.accept_waveform(SAMPLE_RATE, audio)
        self._recognizer.decode_stream(stream)
        return stream.result.text.strip()

    def recognize(self, audio, *, final: bool = True) -> str:
        """The text of `audio`, the parts (cut at pauses) closed by `close_part` and joined.

        The engine's own 、/。 at the end of a part depends on how the audio happened to end,
        not on the sentence, so it is dropped and the mark is decided from the words; only at
        the end of a `final` recording is the engine's mark kept."""
        parts = []
        self.prepare_ms = 0
        for part in split_long(audio):
            if len(part) < SAMPLE_RATE // 10:
                continue
            began = time.perf_counter()
            span = speech_span(part)
            self.prepare_ms += round((time.perf_counter() - began) * 1000)
            if span is not None:
                parts.append(self._decode(part[span[0]:span[1]]))
        if self.english:
            # Words are separated by spaces; the model writes its own marks.
            return " ".join(part for part in parts if part)
        last = len(parts) - 1
        return "".join(
            close_part(part if final and index == last else part.rstrip("、。"))
            for index, part in enumerate(parts)
        )


# By how the words end when a pause comes (content, never the length of the pause):
# a sentence has ended -> 。; a clause goes on with the next words -> 、; a phrase is still
# open (the next words belong to it) -> no mark; anything else -> 、.
SENTENCE_ENDINGS = ("です", "ます", "ました", "でした", "ません", "ください", "でしょう", "ましょう",
                    "ですね", "ますね", "ですよ", "ますよ", "ですか", "ますか", "だ", "よね",
                    "と思う", "たい", "ない", "した", "ている")
CLAUSE_ENDINGS = ("けれども", "けれど", "けど", "ですが", "ますが", "のに", "ので", "から", "ても",
                  "たら", "れば", "ながら", "んで", "いで", "て", "と")
OPEN_PHRASE_ENDINGS = ("の", "を", "に", "へ", "は", "も", "が", "で")
MARKS = "、。？！」』）"


def close_part(text: str) -> str:
    """A part ends at a pause, and these models often write no mark there (Whisper does):
    joined, the parts ran together, and the transcript's end had no 。 (2026-10-23).
    Half-width ? and ! become full-width, as in the rest of the Japanese text."""
    text = text.replace("?", "？").replace("!", "！")
    if not text or text[-1] in MARKS or text[-1].isascii():
        return text
    if text.endswith(SENTENCE_ENDINGS):
        return text + "。"
    if text.endswith(CLAUSE_ENDINGS):
        return text + "、"
    if text.endswith(OPEN_PHRASE_ENDINGS):
        return text
    return text + "、"


def split_long(audio, max_seconds: float = MAX_PART_SECONDS):
    """Parts of at most `max_seconds`, cut in the middle of a pause where there is one."""
    from voiceweave_worker.streaming import find_cut

    limit = int(max_seconds * SAMPLE_RATE)
    parts = []
    while len(audio) > limit:
        cut = find_cut(audio[:limit], min_chunk=5.0, notice=0.0) or limit
        parts.append(audio[:cut])
        audio = audio[cut:]
    parts.append(audio)
    return parts


def load(engine: str, folder: Path, threads: int = 4) -> SherpaModel:
    if engine not in ENGINES:
        raise ValueError("engine")
    if not files_present(engine, folder):
        raise FileNotFoundError("model files")
    return SherpaModel(engine, folder, threads)
