"""Draw the VoiceWeave icon at every size it is shown (numpy only, no image library).

Design (chosen with the user, 2026-10-06, "L2"): a light silver rounded tile with a dark
edge. Two V strokes overlap into a W; the left V (near-black) lies in front of the right
V (grey), so the mark reads as "V" first and "W" as a whole: the voice woven in.
Coordinates are on a 64-unit grid. The 16 px size gets a thicker edge so the tile still
stands out on a light taskbar; every other size uses the thin edge.

Outputs (then nothing else needs regenerating by hand):
  src-tauri/icons/source/voiceweave-1024.png   source for `npm.cmd run tauri -- icon`
  src-tauri/icons/icon.ico                      per-size frames (16 px drawn thick)
  src-tauri/icons/tray-{16,20,24,32}.rgba       raw RGBA for the tray, picked by DPI
Run order: `npm.cmd run tauri -- icon src-tauri/icons/source/voiceweave-1024.png`,
then this script again with --ico-only to put the hand-tuned icon.ico back.
"""
from __future__ import annotations

import argparse
import struct
import zlib
from pathlib import Path

import numpy as np

TILE = (0xE4, 0xE7, 0xEB)
EDGE = (0x15, 0x17, 0x1B)
FRONT = (0x15, 0x17, 0x1B)
BACK = (0x6A, 0x73, 0x80)
LEFT_V = [(10, 18), (23, 46), (36, 18)]
RIGHT_V = [(28, 18), (41, 46), (54, 18)]
STROKE = 6.5
HALO = 12.0  # gap cut around the front V where it crosses the back one
THIN_EDGE, THICK_EDGE = 2.0, 4.5

ICONS = Path(__file__).resolve().parents[1] / "src-tauri" / "icons"


def polyline_distance(x, y, points):
    best = np.full(x.shape, np.inf, np.float32)
    for (ax, ay), (bx, by) in zip(points, points[1:]):
        px, py, dx, dy = x - ax, y - ay, bx - ax, by - ay
        t = np.clip((px * dx + py * dy) / (dx * dx + dy * dy), 0.0, 1.0)
        best = np.minimum(best, np.hypot(px - t * dx, py - t * dy))
    return best


def rounded_rect_distance(x, y, inset, radius):
    half = 32 - inset - radius
    qx, qy = np.abs(x - 32) - half, np.abs(y - 32) - half
    return np.hypot(np.maximum(qx, 0), np.maximum(qy, 0)) + np.minimum(np.maximum(qx, qy), 0) - radius


def draw(size: int, edge: float) -> np.ndarray:
    """RGBA image of `size` px, supersampled so small sizes stay crisp and smooth."""
    ss = max(2, 256 // size)
    n = size * ss
    unit = 64 / n
    y, x = (np.mgrid[0:n, 0:n].astype(np.float32) + 0.5) * unit

    def cover(distance):  # distance in grid units, negative inside
        return np.clip(0.5 - distance / unit, 0.0, 1.0)

    outer = rounded_rect_distance(x, y, 2, 14)
    alpha = cover(outer)
    color = np.zeros((n, n, 3), np.float32) + np.array(EDGE, np.float32)

    def paint(mask, rgb):
        nonlocal color
        m = mask[..., None]
        color = color * (1 - m) + np.array(rgb, np.float32) * m

    paint(cover(outer + edge), TILE)  # the tile inside its dark edge
    paint(cover(polyline_distance(x, y, RIGHT_V) - STROKE / 2), BACK)
    front = polyline_distance(x, y, LEFT_V)
    paint(cover(front - HALO / 2) * cover(outer + edge), TILE)
    paint(cover(front - STROKE / 2), FRONT)

    rgba = np.dstack([color * alpha[..., None], alpha * 255])  # premultiplied
    small = rgba.reshape(size, ss, size, ss, 4).mean(axis=(1, 3))
    a = small[..., 3:4]
    small[..., :3] = np.where(a > 0, small[..., :3] * 255 / np.maximum(a, 1e-6), 0)
    return np.clip(np.round(small), 0, 255).astype(np.uint8)


def edge_for(size: int) -> float:
    return THICK_EDGE if size <= 16 else THIN_EDGE


def png_bytes(rgba: np.ndarray) -> bytes:
    height, width, _ = rgba.shape
    raw = b"".join(b"\x00" + rgba[row].tobytes() for row in range(height))

    def chunk(kind: bytes, data: bytes) -> bytes:
        return (struct.pack(">I", len(data)) + kind + data
                + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF))

    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9))
            + chunk(b"IEND", b""))


def ico_bytes(sizes: list[int]) -> bytes:
    """ICO with one PNG-compressed frame per size (Windows Vista and later)."""
    frames = [png_bytes(draw(size, edge_for(size))) for size in sizes]
    header = struct.pack("<HHH", 0, 1, len(frames))
    offset = 6 + 16 * len(frames)
    entries = b""
    for size, frame in zip(sizes, frames):
        entries += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(frame), offset)
        offset += len(frame)
    return header + entries + b"".join(frames)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--ico-only", action="store_true", help="only rewrite icon.ico and tray images")
    args = parser.parse_args()
    if not args.ico_only:
        source = ICONS / "source" / "voiceweave-1024.png"
        source.parent.mkdir(parents=True, exist_ok=True)
        source.write_bytes(png_bytes(draw(1024, THIN_EDGE)))
        print(source)
    (ICONS / "icon.ico").write_bytes(ico_bytes([16, 20, 24, 32, 40, 48, 64, 128, 256]))
    for size in (16, 20, 24, 32):
        (ICONS / f"tray-{size}.rgba").write_bytes(draw(size, edge_for(size)).tobytes())
    preview = ICONS / "source" / "preview-16.png"
    preview.write_bytes(png_bytes(draw(16, THICK_EDGE)))
    print("icon.ico and tray images written")


if __name__ == "__main__":
    main()
