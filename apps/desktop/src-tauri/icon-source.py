"""Generate the placeholder app icon (1024x1024 PNG, a gauge arc on a rounded square).

Run: python3 icon-source.py icon-source.png && pnpm tauri icon icon-source.png
"""

import math
import struct
import sys
import zlib

SIZE = 1024
RADIUS = 220
BG = (24, 28, 38)
ARC = (96, 200, 160)
TRACK = (52, 60, 78)


def in_rounded_square(x: int, y: int) -> bool:
    cx = min(max(x, RADIUS), SIZE - 1 - RADIUS)
    cy = min(max(y, RADIUS), SIZE - 1 - RADIUS)
    return (x - cx) ** 2 + (y - cy) ** 2 <= RADIUS**2


def pixel(x: int, y: int) -> tuple[int, int, int, int]:
    if not in_rounded_square(x, y):
        return (0, 0, 0, 0)
    dx, dy = x - SIZE / 2, y - SIZE * 0.58
    dist = math.hypot(dx, dy)
    angle = math.degrees(math.atan2(-dy, dx))  # 0 = right, 90 = up
    if 300 <= dist <= 380 and -30 <= angle <= 210:
        # The arc runs from 210 (left) over 90 (top) to -30 (right); it is filled up to 40.
        return (*(ARC if angle >= 40 else TRACK), 255)
    return (*BG, 255)


def main(path: str) -> None:
    rows = bytearray()
    for y in range(SIZE):
        rows.append(0)
        for x in range(SIZE):
            rows.extend(pixel(x, y))

    def chunk(tag: bytes, data: bytes) -> bytes:
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(bytes(rows), 9))
    png += chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(png)


if __name__ == "__main__":
    main(sys.argv[1])
