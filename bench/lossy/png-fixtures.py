#!/usr/bin/env python3
"""Prepare the pinned PNG quantization validation set. Requires Pillow with Little CMS."""
import argparse
import hashlib
import struct
from io import BytesIO
from pathlib import Path
from urllib.request import urlopen

from PIL import Image, ImageCms, PngImagePlugin

BASE = "https://raw.githubusercontent.com/scikit-image/scikit-image/v0.25.2/skimage/data/"
HASHES = {
    "brick.png": "7966caf324f6ba843118d98f7a07746d22f6a343430add0233eca5f6eaaa8fcf",
    "cell.png": "8d23a7fb81f7cc877cd09f330357fc7f595651306e84e17252f6e0a1b3f61515",
    "chelsea.png": "596aa1e7cb875eb79f437e310381d26b338a81c2da23439704a73c4651e8c4bb",
    "coffee.png": "cc02f8ca188b167c775a7101b5d767d1e71792cf762c33d6fa15a4599b5a8de7",
    "color.png": "7d2df993de2b4fa2a78e04e5df8050f49a9c511aa75e59ab3bd56ac9c98aef7e",
    "grass.png": "b6b6022426b38936c43a4ac09635cd78af074e90f42ffa8227ac8b7452d39f89",
    "gravel.png": "c48615b451bf1e606fbd72c0aa9f8cc0f068ab7111ef7d93bb9b0f2586440c12",
    "ihc.png": "f8dd1aa387ddd1f49d8ad13b50921b237df8e9b262606d258770687b0ef93cef",
    "moon.png": "78739619d11f7eb9c165bb5d2efd4772cee557812ec847532dbb1d92ef71f577",
    "phantom.png": "552ff698167aa402cceb17981130607a228a0a0aa7c519299eaa4d5f301ba36c",
    "rocket.jpg": "c2dd0de7c538df8d111e479619b129464d0269d0ae5fd18ca91d33a7fdfea95c",
    "text.png": "bd84aa3a6e3c9887850d45d606c96b2e59433fbef50338570b63c319e668e6d1",
}


def profile(minute, second):
    data = bytearray(ImageCms.ImageCmsProfile(ImageCms.createProfile("sRGB")).tobytes())
    # Hold the creation timestamp fixed across repeated fixture generation.
    data[24:36] = struct.pack(">6H", 2026, 11, 20, 15, minute, second)
    return bytes(data)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("out", type=Path, help="new or empty directory")
    parser.add_argument("--cache", type=Path, help="optional local directory containing upstream originals")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    if any(args.out.iterdir()):
        parser.error("The output directory must be empty")
    origin = PngImagePlugin.PngInfo()
    origin.add_text("Origin", "lossy validation")
    for name, sha in HASHES.items():
        if args.cache:
            data = (args.cache / name).read_bytes()
        else:
            with urlopen(BASE + name, timeout=30) as response:
                data = response.read()
        if hashlib.sha256(data).hexdigest() != sha:
            raise ValueError(f"Source checksum mismatch for {name}")
        with Image.open(BytesIO(data)) as original:
            side = min(*original.size, 208)
            left, top = ((n - side) // 2 for n in original.size)
            crop = original.crop((left, top, left + side, top + side))
            crop.info.clear()
            crop.save(args.out / (Path(name).stem + ".png"), pnginfo=origin,
                      icc_profile=profile(0, 10) if crop.mode == "RGB" else None)
    alpha = Image.new("RGBA", (384, 128))
    alpha.putdata([(17, 49, 91, 0) if y < 32 else
                   (x // 2 + y // 2, x // 4 + y // 2, x // 3 + y // 3, 160 if y < 64 else 255)
                   for y in range(alpha.height) for x in range(alpha.width)])
    alpha.save(args.out / "alpha.png", icc_profile=profile(0, 10))
    tagged = Image.new("RGBA", (192, 128))
    tagged.putdata([(23, 18, 91, 0) if y < 8 else
                    (20 + x // 2 + y // 4, 12 + x // 6 + y // 2, 40 + x // 4 + y // 6,
                     97 if y < 64 else 255) for y in range(tagged.height) for x in range(tagged.width)])
    title = PngImagePlugin.PngInfo()
    title.add_text("Title", "Opacity and ICC")
    tagged.save(args.out / "rgba-metadata.png", compress_level=0, pnginfo=title,
                icc_profile=profile(7, 28), dpi=(96, 96))


if __name__ == "__main__":
    main()
