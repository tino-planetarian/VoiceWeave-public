"""Deliberately malformed worker replies for a Rust process-boundary probe."""

import json
import struct
import sys
import time

MAX_HEADER = 65_536


def send(kind: str, request_id: str | None, **fields: object) -> None:
    header = json.dumps(
        {"v": 1, "kind": kind, "request_id": request_id, "payload_bytes": 0, **fields}
    ).encode("utf-8")
    sys.stdout.buffer.write(struct.pack("<I", len(header)) + header)
    sys.stdout.buffer.flush()


def read_request() -> dict:
    prefix = sys.stdin.buffer.read(4)
    if len(prefix) != 4:
        raise SystemExit(2)
    size = struct.unpack("<I", prefix)[0]
    header = json.loads(sys.stdin.buffer.read(size))
    if header["payload_bytes"]:
        sys.stdin.buffer.read(header["payload_bytes"])
    return header


def main() -> None:
    scenario = sys.argv[1]
    if scenario == "bad_hello":
        send("hello", None)
        return
    if scenario == "hello_hang":
        time.sleep(30)
        return
    send("hello", None, worker_version="fake-1")
    if scenario == "eof":
        return
    request = read_request()
    if scenario in ("transcribe_eof", "transcribe_recover", "transcribe_measured"):
        if request["kind"] != "prepare":
            raise SystemExit(4)
        send("ready", request["request_id"])
        request = read_request()
        if request["kind"] != "transcribe":
            raise SystemExit(4)
        if scenario == "transcribe_eof":
            return
        send(
            "result",
            request["request_id"],
            session_id=request["session_id"],
            raw_text="",
            **({"elapsed_ms": 9000, "frontend_ms": 200, "decode_ms": 8800}
               if scenario == "transcribe_measured" else {}),
        )
        request = read_request()
        if request["kind"] != "shutdown":
            raise SystemExit(4)
        send("bye", request["request_id"])
        return
    if scenario == "wrong_id":
        send("bye", "another-request")
    elif scenario == "oversize":
        sys.stdout.buffer.write(struct.pack("<I", MAX_HEADER + 1))
        sys.stdout.buffer.flush()
    elif scenario == "shutdown_hang":
        time.sleep(30)
    elif scenario == "stderr_flood":
        sys.stderr.buffer.write(b"x" * 1_000_000)
        sys.stderr.buffer.flush()
        send("bye", request["request_id"])
    else:
        raise SystemExit(3)


if __name__ == "__main__":
    main()
