from io import BytesIO
import struct

import pytest

from voiceweave_worker.protocol import MAX_HEADER, ProtocolError, read_frame, write_frame


def test_frame_survives_chunked_reads() -> None:
    output = BytesIO()
    write_frame(output, {"kind": "transcribe", "request_id": "r1"}, b"\x00\x01\x02\x03")

    class Chunked(BytesIO):
        def read(self, size: int = -1) -> bytes:
            return super().read(min(size, 1))

    header, payload = read_frame(Chunked(output.getvalue()))  # type: ignore[misc]
    assert header["kind"] == "transcribe"
    assert payload == b"\x00\x01\x02\x03"


def test_oversized_header_is_rejected_before_allocation() -> None:
    with pytest.raises(ProtocolError, match="header length"):
        read_frame(BytesIO(struct.pack("<I", MAX_HEADER + 1)))


def test_truncated_body_is_rejected() -> None:
    output = BytesIO()
    write_frame(output, {"kind": "transcribe", "request_id": "r1"}, b"abc")
    with pytest.raises(ProtocolError, match="truncated"):
        read_frame(BytesIO(output.getvalue()[:-1]))


def test_eof_between_frames_is_normal() -> None:
    assert read_frame(BytesIO()) is None
