"""Versioned stdio frames shared with the Rust host.

Every message starts with a little-endian u32 JSON header length, then the JSON
header and an optional binary body. stdout is exclusively reserved for frames.
"""

from __future__ import annotations

import json
import struct
from typing import Any, BinaryIO

VERSION = 1
MAX_HEADER = 65_536
MAX_PAYLOAD = 19_200_000


class ProtocolError(Exception):
    pass


def _read_exact(stream: BinaryIO, length: int) -> bytes:
    parts: list[bytes] = []
    remaining = length
    while remaining:
        chunk = stream.read(remaining)
        if not chunk:
            raise ProtocolError("truncated frame")
        parts.append(chunk)
        remaining -= len(chunk)
    return b"".join(parts)


def read_frame(stream: BinaryIO) -> tuple[dict[str, Any], bytes] | None:
    prefix = stream.read(1)
    if prefix == b"":
        return None
    prefix += _read_exact(stream, 3)
    (length,) = struct.unpack("<I", prefix)
    if length == 0 or length > MAX_HEADER:
        raise ProtocolError("invalid header length")
    try:
        header = json.loads(_read_exact(stream, length).decode("utf-8"))
    except (UnicodeError, ValueError) as exc:
        raise ProtocolError("invalid JSON header") from exc
    if not isinstance(header, dict):
        raise ProtocolError("header is not an object")
    if header.get("v") != VERSION or type(header.get("v")) is not int:
        raise ProtocolError("unsupported protocol version")
    kind = header.get("kind")
    if not isinstance(kind, str) or not kind:
        raise ProtocolError("invalid message kind")
    request_id = header.get("request_id")
    if request_id is not None and (not isinstance(request_id, str) or not request_id):
        raise ProtocolError("invalid request ID")
    payload_bytes = header.get("payload_bytes")
    if type(payload_bytes) is not int or not 0 <= payload_bytes <= MAX_PAYLOAD:
        raise ProtocolError("invalid payload length")
    return header, _read_exact(stream, payload_bytes)


def write_frame(stream: BinaryIO, header: dict[str, Any], payload: bytes = b"") -> None:
    if not isinstance(payload, bytes) or len(payload) > MAX_PAYLOAD:
        raise ProtocolError("invalid payload")
    data = dict(header)
    data["v"] = VERSION
    data["payload_bytes"] = len(payload)
    if "kind" not in data or "request_id" not in data:
        raise ProtocolError("kind and request_id are required")
    raw = json.dumps(data, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    if not raw or len(raw) > MAX_HEADER:
        raise ProtocolError("invalid header length")
    stream.write(struct.pack("<I", len(raw)))
    stream.write(raw)
    if payload:
        stream.write(payload)
    stream.flush()
