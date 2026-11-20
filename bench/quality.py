#!/usr/bin/env python3
"""Independently compare decoded samples, transparency and metadata after opt-in image compression."""

import argparse
import json
import math
import struct
import subprocess
from pathlib import Path

import verify


def png_quality(original, candidate):
    from PIL import Image

    def read(path):
        data = path.read_bytes()
        fields = list(verify.chunks(data))
        header = struct.unpack(">IIBBBBB", fields[0][1])
        metadata = [(k, v) for k, v in fields
                    if k not in {b"IHDR", b"PLTE", b"tRNS", b"IDAT", b"IEND"}]
        return data, header, metadata, any(k == b"PLTE" for k, _ in fields)

    a, src, metadata, has_palette = read(original)
    b, dst, preserved, _ = read(candidate)
    if src[:2] != dst[:2]:
        raise ValueError("Changed PNG dimensions")
    report = dict(file=original.name, source_bytes=len(a), output_bytes=len(b),
                  source_sha256=verify.digest(a), output_sha256=verify.digest(b),
                  frames=1, peak_channel_error=0, worst_channel_psnr_db=None,
                  rgb_psnr_db=None, luma_mse=0.0, max_tile_luma_mse=0.0,
                  percent_over_24=0.0)
    for kind, value in metadata:
        if kind == b"acTL":
            report["frames"] = int.from_bytes(value[:4], "big")
    if a == b:
        return report
    if verify.requires_identity_data(a):
        raise ValueError("Changed byte-sensitive PNG")
    # The CLI may choose the strict lossless winner for an ineligible image.
    # Never flatten an animation or truncate 16-bit samples for verification.
    allowed = {b"cHRM", b"gAMA", b"iCCP", b"iTXt", b"pHYs", b"sRGB", b"tEXt", b"tIME", b"zTXt"}
    if (src[2] != 8 or src[6] or has_palette or dst[3] != 3
            or dst[2] not in (1, 2, 4, 8) or dst[6] or metadata != preserved
            or src[0] * src[1] > 12_000_000 or any(k not in allowed for k, _ in metadata)):
        if verify.signature(original) != verify.signature(candidate):
            raise ValueError("Changed PNG outside the quantizer scope")
        return report
    w, h = src[:2]
    if w * h > 20_000_000:
        raise ValueError("Exceeded PNG validation pixel limit")
    with Image.open(original) as before, Image.open(candidate) as after:
        pixels = before.convert("RGBA").tobytes()
        mapped = after.convert("RGBA").tobytes()
    peak = tail = squared_luma = 0
    sums = [0, 0, 0]
    tile_count = (w + 31) // 32
    tile_sums = [0] * tile_count
    max_tile = 0.0
    for y in range(h):
        for x in range(w):
            i = 4 * (y * w + x)
            if pixels[i + 3] != mapped[i + 3] or (
                    pixels[i + 3] == 0 and pixels[i:i + 3] != mapped[i:i + 3]):
                raise ValueError("Changed PNG alpha or hidden RGB")
            diff = [pixels[i + c] - mapped[i + c] for c in range(3)]
            for c, d in enumerate(diff):
                sums[c] += d * d
            local_peak = max(map(abs, diff))
            peak = max(peak, local_peak)
            tail += local_peak > 24
            dy = 54 * diff[0] + 183 * diff[1] + 19 * diff[2]
            squared_luma += dy * dy
            tile_sums[x // 32] += dy * dy
        if y % 32 == 31 or y == h - 1:
            for j, total in enumerate(tile_sums):
                tile_pixels = (y % 32 + 1) * min(32, w - 32 * j)
                max_tile = max(max_tile, total / (65536 * tile_pixels))
            tile_sums = [0] * tile_count
    n = w * h
    if peak > 64 or tail * 100 > n or sum(sums) > 60 * n or squared_luma > 12 * 65536 * n or max_tile > 49:
        raise ValueError("PNG exceeds its bounded-error profile")
    def psnr(sse, count):
        return round(10 * math.log10(255**2 * count / sse), 4) if sse else None
    report.update(peak_channel_error=peak, worst_channel_psnr_db=psnr(max(sums), n),
                  rgb_psnr_db=psnr(sum(sums), 3 * n), luma_mse=round(squared_luma / (65536 * n), 4),
                  max_tile_luma_mse=round(max_tile, 4), percent_over_24=round(100 * tail / n, 4))
    return report


def representations(path):
    image = path.read_bytes()
    if image[:2] == b"\xff\xd8":
        dimensions = subprocess.check_output(
            [
                "ffprobe",
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=width,height,pix_fmt",
                "-of",
                "json",
                str(path),
            ]
        )
        return (verify.jpeg_info(image), json.loads(dimensions)), [
            verify.ffmpeg_frames(path, raw_eight=True)
        ]
    if image.startswith((b"GIF87a", b"GIF89a")):
        properties = verify.gif_semantics(image, path, raw=True)
        frames = properties.pop("raw_frames")
        properties.pop("pixels_rgba16")
        return (properties, [f[:4] for f in frames]), [f[-1] for f in frames]
    if image[8:12] == b"WEBP":
        properties = verify.webp_semantics(image, path, raw=True)
        frames = properties.pop("pixels_rgba16")
        if isinstance(frames, bytes):
            return properties, [frames]
        return (properties, [f[0] for f in frames]), [f[1] for f in frames]
    raise ValueError("The bounded-error oracle supports JPEG, GIF and WebP")


def compare(original, candidate):
    if original.suffix.lower() == ".png":
        return png_quality(original, candidate)
    before, source_frames = representations(original)
    after, output_frames = representations(candidate)
    if before != after or len(source_frames) != len(output_frames):
        raise ValueError("Changed frame geometry, controls or metadata")
    peak = 0
    worst_channel = None
    for source, result in zip(source_frames, output_frames):
        if len(source) != len(result) or len(source) % 4:
            raise ValueError("Changed number of decoded samples")
        sums = [0, 0, 0]
        for i in range(0, len(source), 4):
            if source[i + 3] != result[i + 3] or (
                source[i + 3] == 0 and source[i : i + 3] != result[i : i + 3]
            ):
                raise ValueError("Changed alpha or color underneath transparent pixels")
            for c in range(3):
                e = abs(source[i + c] - result[i + c])
                peak = max(peak, e)
                sums[c] += e * e
        for ss in sums:
            if ss:
                value = 10 * math.log10(255 * 255 * (len(source) // 4) / ss)
                worst_channel = (
                    value if worst_channel is None else min(worst_channel, value)
                )
    return dict(
        file=original.name,
        source_bytes=original.stat().st_size,
        output_bytes=candidate.stat().st_size,
        source_sha256=verify.digest(original.read_bytes()),
        output_sha256=verify.digest(candidate.read_bytes()),
        frames=len(source_frames),
        peak_channel_error=peak,
        worst_channel_psnr_db=None
        if worst_channel is None
        else round(worst_channel, 4),
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--originals", type=Path, required=True)
    parser.add_argument("--outputs", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    if args.out.exists():
        parser.error("Use a fresh report file")
    files = [
        p
        for p in sorted(args.outputs.iterdir())
        if p.suffix.lower() in (".jpg", ".jpeg", ".gif", ".webp", ".png")
    ]
    if not files:
        parser.error("No images to compare")
    rows = [compare(args.originals / p.name, p) for p in files]
    args.out.write_text(json.dumps(rows, indent=2) + "\n")


if __name__ == "__main__":
    main()
