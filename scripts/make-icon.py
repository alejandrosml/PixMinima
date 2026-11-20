"""Render the three diagonal strokes used by the application's own wordmark."""
import math
from pathlib import Path
import struct
import sys
import zlib


def segment_distance(x, y, start_x):
    dx, dy = 9.0, -12.0
    t = max(0.0, min(1.0, ((x-start_x)*dx + (y-22.0)*dy)/(dx*dx+dy*dy)))
    return math.hypot(x-start_x-t*dx, y-22.0-t*dy)


def chunk(tag, data):
    return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag+data))


def draw(n, file):
    raw = bytearray()
    # A one-pixel antialiased edge keeps all ten macOS icon sizes crisp.
    step = 36.0 / n
    for iy in range(n):
        raw.append(0)
        for ix in range(n):
            x, y = (ix + 0.5)*step - 2, (iy + 0.5)*step - 2
            qx, qy = abs(x-16)-9, abs(y-16)-9
            sdf = math.hypot(max(0,qx), max(0,qy)) + min(max(qx,qy),0) - 7
            a = max(0, min(1, 0.5-sdf/step))
            if a == 0:
                raw.extend((0,0,0,0))
                continue
            distances = [segment_distance(x,y,s) for s in (6.5,11.5,16.5)]
            index = distances.index(min(distances))
            stroke = distances[index] - 1.35
            mix = max(0,min(1,0.5-stroke/step))
            ink = ((255,253,247),(255,210,63),(255,106,98))[index]
            for base, highlight in zip((37,88,200), ink):
                raw.append(round(base+(highlight-base)*mix))
            raw.append(round(255*a))
    header = struct.pack(">IIBBBBB", n, n, 8, 6, 0, 0, 0)
    file.write_bytes(b"\x89PNG\r\n\x1a\n"+chunk(b"IHDR",header)+chunk(b"IDAT",zlib.compress(raw,9))+chunk(b"IEND",b""))


if __name__ == "__main__":
    directory = Path(sys.argv[1])
    directory.mkdir(parents=True, exist_ok=True)
    for size in (16, 32, 128, 256, 512):
        for scale, suffix in ((1,""),(2,"@2x")):
            draw(size*scale, directory/f"icon_{size}x{size}{suffix}.png")
