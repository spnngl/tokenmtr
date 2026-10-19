"""Generate the tray icons: the TokenMtr mark, black on transparent for macOS, white for Linux.

The geometry is assets/brand/tokenmtr-logo.svg (100-unit viewBox): two arcs open at the top
right, a hub ring and a needle, all 6 units wide.

macOS tints a template image by its alpha channel only, so every pixel is black and the shape
lives in the alpha. Writes `tray-icon.png` (22x22) and `tray-icon@2x.png` (44x44) into DIR.
Linux has no template rendering and its panels are mostly dark, so it gets `tray-icon-light.png`
(44x44), the same mark in white.

Run: python3 tray-icon.py icons
"""

import math
import struct
import sys
import zlib
from pathlib import Path

SAMPLES = 8  # supersampling per axis, for anti-aliased edges


HALF_STROKE = 3.0
# The mark spans 11..89 units on both axes (outer arc radius 36 plus half a stroke).
MARK_MIN, MARK_SPAN = 11.0, 78.0
# Ring radius and the open angle range (degrees, 0 = right, 90 = up) of each arc.
ARCS = ((36.0, 20.0, 70.0), (22.0, 15.0, 75.0))
HUB_RADIUS = 9.0
NEEDLE = ((56.4, 43.6), (83.2, 16.8))


def coverage(x: float, y: float, size: int) -> bool:
    """Whether the point (in pixels) lies on the mark."""
    padding = size / 44  # half a pixel at 22 px
    scale = MARK_SPAN / (size - 2 * padding)
    ux = MARK_MIN + (x - padding) * scale
    uy = MARK_MIN + (y - padding) * scale
    dx, dy = ux - 50.0, uy - 50.0
    dist = math.hypot(dx, dy)
    angle = math.degrees(math.atan2(-dy, dx))
    for radius, gap_from, gap_to in ARCS:
        if abs(dist - radius) <= HALF_STROKE and not gap_from < angle < gap_to:
            return True
    if abs(dist - HUB_RADIUS) <= HALF_STROKE:
        return True
    (x1, y1), (x2, y2) = NEEDLE
    length = math.hypot(x2 - x1, y2 - y1)
    tx, ty = (x2 - x1) / length, (y2 - y1) / length
    along = (ux - x1) * tx + (uy - y1) * ty
    across = abs(-(ux - x1) * ty + (uy - y1) * tx)
    return 0 <= along <= length and across <= HALF_STROKE


def alpha(px: int, py: int, size: int) -> int:
    hits = sum(
        coverage(px + (sx + 0.5) / SAMPLES, py + (sy + 0.5) / SAMPLES, size)
        for sx in range(SAMPLES)
        for sy in range(SAMPLES)
    )
    return round(255 * hits / SAMPLES**2)


def png(size: int, shade: int = 0) -> bytes:
    rows = bytearray()
    for y in range(size):
        rows.append(0)
        for x in range(size):
            rows.extend((shade, shade, shade, alpha(x, y, size)))

    def chunk(tag: bytes, data: bytes) -> bytes:
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(bytes(rows), 9))
        + chunk(b"IEND", b"")
    )


def main(directory: str) -> None:
    for size, name in ((22, "tray-icon.png"), (44, "tray-icon@2x.png")):
        Path(directory, name).write_bytes(png(size))
    Path(directory, "tray-icon-light.png").write_bytes(png(44, 255))


if __name__ == "__main__":
    main(sys.argv[1])
