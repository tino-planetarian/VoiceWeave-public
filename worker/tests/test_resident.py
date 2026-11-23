"""Keeping the recognizer in memory: the minimum asked for, and a real call on Windows."""
import sys

from voiceweave_worker import resident


def test_the_minimum_stays_within_the_floor_and_the_cap():
    assert resident.minimum_for(0) == resident.FLOOR_BYTES
    assert resident.minimum_for(1024**3) == 1024**3
    assert resident.minimum_for(10 * 1024**3) == resident.CAP_BYTES


def test_the_request_never_fails_loudly():
    result = resident.keep_resident()
    if sys.platform == "win32":
        # Windows grants it to ordinary users; a refusal must still be quiet.
        assert result is None or result >= resident.FLOOR_BYTES
    else:
        assert result is None
