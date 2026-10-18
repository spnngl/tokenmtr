"""Generate the macOS menu-bar template icon: a gauge arc with a needle, black on transparent.

macOS tints a template image by its alpha channel only, so every pixel is black and the shape
lives in the alpha. Writes `tray-icon.png` (22x22) and `tray-icon@2x.png` (44x44) into DIR.

Run: python3 tray-icon.py icons
"""

import math
import struct
import sys
import zlib
from pathlib import Path

SAMPLES = 8  # supersampling per axis, for anti-aliased edges


def coverage(x: float, y: float, size: int) -> bool:
    """Whether the point (in pixels) lies on the arc or the needle."""
    scale = size / 22
    cx, cy = 11 * scale, 12.5 * scale
    dx, dy = x - cx, y - cy
    dist = math.hypot(dx, dy)
    angle = math.degrees(math.atan2(-dy, dx))  # 0 = right, 90 = up
    on_ring = 7.0 * scale <= dist <= 9.5 * scale
    # The arc runs from 210 (left) over 90 (top) to -30 (right); the bottom is open.
    if on_ring and (angle >= -30 or angle <= -150):
        return True
    # The needle points up and to the right, ending in a hub.
    needle = math.radians(55)
    ux, uy = math.cos(needle), -math.sin(needle)
    along = dx * ux + dy * uy
    across = abs(-dx * uy + dy * ux)
    if 0 <= along <= 6.0 * scale and across <= 0.9 * scale:
        return True
    return dist <= 1.8 * scale


def alpha(px: int, py: int, size: int) -> int:
    hits = sum(
        coverage(px + (sx + 0.5) / SAMPLES, py + (sy + 0.5) / SAMPLES, size)
        for sx in range(SAMPLES)
        for sy in range(SAMPLES)
    )
    return round(255 * hits / SAMPLES**2)


def png(size: int) -> bytes:
    rows = bytearray()
    for y in range(size):
        rows.append(0)
        for x in range(size):
            rows.extend((0, 0, 0, alpha(x, y, size)))

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


if __name__ == "__main__":
    main(sys.argv[1])
