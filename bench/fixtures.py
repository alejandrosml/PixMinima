"""Hash-pinned upstream images and deterministic coverage fixtures."""

from pathlib import Path
import gzip
import hashlib
import json
import struct
import subprocess
import urllib.request
import zlib
import zipfile
from verify import chunks

HERE = Path(__file__).resolve().parent
MANIFEST = json.loads((HERE / "corpus.json").read_text())


def chunk(kind, body):
    return (
        struct.pack(">I", len(body))
        + kind
        + body
        + struct.pack(">I", zlib.crc32(kind + body) & 0xFFFFFFFF)
    )


def make_png(w, h, depth, color, scanlines, extras=()):
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, depth, color, 0, 0, 0))
        + b"".join(chunk(t, v) for t, v in extras)
        + chunk(b"IDAT", zlib.compress(scanlines, 0))
        + chunk(b"IEND", b"")
    )


def prepare(root, sources, with_extended=False):
    root.mkdir(parents=True, exist_ok=True)
    marker = root / ".pixminima-benchmark"
    if not marker.exists() and any(root.iterdir()):
        raise ValueError("Use a fresh or previously prepared benchmark directory")
    if not marker.exists():
        marker.write_text("PixMinima benchmark corpus v1\n")
    primary = root / "primary"
    coverage = root / "coverage"
    primary.mkdir(parents=True, exist_ok=True)
    coverage.mkdir(parents=True, exist_ok=True)
    collections = [(primary, MANIFEST["files"])]
    if with_extended:
        extended = root / "extended"
        extended.mkdir(exist_ok=True)
        collections.append((extended, MANIFEST["extended"]))
    for out, sha in [
        (folder / name, sha)
        for folder, entries in collections
        for name, sha in entries.items()
    ]:
        name = out.name
        if out.is_file():
            data = out.read_bytes()
        else:
            local = next((p / name for p in sources if (p / name).is_file()), None)
            data = (
                local.read_bytes()
                if local
                else urllib.request.urlopen(MANIFEST["base"] + name, timeout=30).read()
            )
        if hashlib.sha256(data).hexdigest() != sha:
            raise ValueError("Input hash mismatch: " + name)
        if not out.exists():
            out.write_bytes(data)
    w, h = 24, 16
    raw = b"".join(
        b"\0"
        + b"".join(
            struct.pack(
                ">4H",
                250 + x * 499,
                1000 + y * 167,
                4097 + (x ^ y),
                0 if x % 3 == 0 else 65535,
            )
            for x in range(w)
        )
        for y in range(h)
    )
    rgba = make_png(w, h, 16, 6, raw, [(b"tEXt", b"Comment\0Lossless bench")])
    (coverage / "rgba16-hidden.png").write_bytes(rgba)

    def add_before_idat(p, t, b):
        at = p.index(b"IDAT") - 4
        return p[:at] + chunk(t, b) + p[at:]

    (coverage / "private-unsafe.png").write_bytes(
        add_before_idat(rgba, b"paNG", b"opaque proprietary offsets")
    )
    (coverage / "private-safe.png").write_bytes(
        add_before_idat(rgba, b"paNg", b"opaque payload")
    )
    eight = bytes([0, 180, 120, 255, 255, 11, 90, 0]) * 12
    sprite = make_png(w, h, 8, 6, (b"\0" + eight) * h)
    (coverage / "icon.ico").write_bytes(
        struct.pack("<HHHBBBBHHII", 0, 1, 1, w, h, 0, 0, 1, 32, len(sprite), 22)
        + sprite
    )
    (coverage / "cursor.cur").write_bytes(
        struct.pack("<HHHBBBBHHII", 0, 2, 1, w, h, 0, 0, 4, 5, len(sprite), 22) + sprite
    )
    svg = b'<svg xmlns="http://www.w3.org/2000/svg" width="32" height="16"><title>Coverage</title><!--retain--><rect x="0" y="0" width="32" height="16" fill="#224466"/><text x="2" y="10">A B</text></svg>'
    (coverage / "vector.svg").write_bytes(svg)
    (coverage / "vector.svgz").write_bytes(gzip.compress(svg, compresslevel=0, mtime=0))
    # BITMAPINFOHEADER, 24-bit BGR, lossless palette/RLE reduction possible.
    bmp_raw = b"".join(
        b"".join(bytes([0, 155 if (x // 4 + y) % 2 else 0, 254]) for x in range(w))
        for y in range(h)
    )
    dib = struct.pack(
        "<IiiHHIIiiII", 40, w, h, 1, 24, 0, len(bmp_raw), 2835, 2835, 0, 0
    )
    (coverage / "stripes.bmp").write_bytes(
        b"BM" + struct.pack("<IHHI", 54 + len(bmp_raw), 0, 0, 54) + dib + bmp_raw
    )
    # Minimal 8-bit TIFF, one strip; include a descriptive ASCII tag.
    text = b"PixMinima coverage\0"
    tags = [
        (256, 4, 1, w),
        (257, 4, 1, h),
        (258, 3, 1, 8),
        (259, 3, 1, 1),
        (262, 3, 1, 1),
        (270, 2, len(text), 0),
        (273, 4, 1, 0),
        (274, 3, 1, 1),
        (277, 3, 1, 1),
        (278, 4, 1, h),
        (279, 4, 1, w * h),
    ]
    start = 8 + 2 + len(tags) * 12 + 4
    fields = b"".join(
        struct.pack(
            "<HHII",
            tag,
            ty,
            n,
            start if tag == 270 else start + len(text) if tag == 273 else v,
        )
        for tag, ty, n, v in tags
    )
    (coverage / "strip.tif").write_bytes(
        b"II*\0"
        + struct.pack("<IH", 8, len(tags))
        + fields
        + b"\0" * 4
        + text
        + bytes((x // 4) * 23 for y in range(h) for x in range(w))
    )
    # Deliberately short LZW strings. Every pixel is preceded by a GIF clear.
    codes = [c for y in range(h) for x in range(w) for c in (4, (x + y) % 2)] + [5]
    bitset = sum(v << (3 * i) for i, v in enumerate(codes)).to_bytes(
        (3 * len(codes) + 7) // 8, "little"
    )
    parts = (
        b"".join(
            bytes([len(bitset[i : i + 255])]) + bitset[i : i + 255]
            for i in range(0, len(bitset), 255)
        )
        + b"\0"
    )
    screen = struct.pack("<HHBBB", w, h, 128, 0, 0) + bytes.fromhex("ff220011CCBB")
    frame = b"\x2c" + struct.pack("<HHHHB", 0, 0, w, h, 0) + b"\x02" + parts

    def gce(delay):
        return b"!\xf9\x04" + struct.pack("<BHB", 5, delay, 0) + b"\0"

    (coverage / "motion.gif").write_bytes(
        b"GIF89a"
        + screen
        + b"!\xfe\x04meta\0"
        + b"!\xff\x0bNETSCAPE2.0\x03\x01\x00\x00\0"
        + gce(3)
        + frame
        + gce(9)
        + frame
        + b";"
    )
    # Compressed VP8L data comes from external libwebp, then retain an XMP chunk.
    base = root / "fixture-base.png"
    base.write_bytes(sprite)
    cp = subprocess.run(
        ["cwebp", "-quiet", "-lossless", "-exact", "-m", "0", str(base), "-o", "-"],
        capture_output=True,
        check=True,
    )

    def riff(t, b):
        return t + struct.pack("<I", len(b)) + b + b"\0" * (len(b) & 1)

    def webp(body):
        return b"RIFF" + struct.pack("<I", len(body) + 4) + b"WEBP" + body

    dims = (w - 1).to_bytes(3, "little") + (h - 1).to_bytes(3, "little")
    body = (
        riff(b"VP8X", bytes([0x14]) + b"\0" * 3 + dims)
        + cp.stdout[12:]
        + riff(b"XMP ", b'<x:xmpmeta xmlns:x="adobe:ns:meta/"/>')
    )
    (coverage / "sprite.webp").write_bytes(webp(body))

    def anim_frame(duration):
        return riff(
            b"ANMF",
            bytes(6)
            + dims
            + duration.to_bytes(3, "little")
            + bytes([0])
            + cp.stdout[12:],
        )

    (coverage / "motion.webp").write_bytes(
        webp(
            riff(b"VP8X", bytes([0x12]) + b"\0" * 3 + dims)
            + riff(b"ANIM", bytes(6))
            + anim_frame(30)
            + anim_frame(90)
        )
    )
    sample = (
        HERE.parent / "crates/archive-jxl/src/fixtures/progressive.jpg"
    ).read_bytes()
    (coverage / "trailing.jpg").write_bytes(sample + b"\0offset-sensitive-trailer")
    iccp = next(
        b for t, b in chunks((primary / "astronaut.png").read_bytes()) if t == b"iCCP"
    )
    exif = bytes.fromhex("49492a0008000000010012010300010000000600000000000000")
    probe = add_before_idat(add_before_idat(sprite, b"iCCP", iccp), b"eXIf", exif)
    (coverage / "metadata-probe.png").write_bytes(probe)
    (coverage / "metadata-probe.jpg").write_bytes(
        sample[:2]
        + b"\xff\xe1"
        + struct.pack(">H", len(exif) + 8)
        + b"Exif\0\0"
        + exif
        + sample[2:]
    )

    # Two frames with a subrectangle, nontrivial alpha and distinct delays.
    def frame(seq, width, height, x, y, delay):
        return struct.pack(">5I2H2B", seq, width, height, x, y, delay, 100, 0, 0)

    head = struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0)
    gray = bytes([5, 99, 210, 0]) * 12
    ap = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", head)
        + chunk(b"acTL", struct.pack(">II", 2, 3))
        + chunk(b"fcTL", frame(0, w, h, 0, 0, 4))
        + chunk(b"IDAT", zlib.compress((b"\0" + eight) * h, 0))
        + chunk(b"fcTL", frame(1, 12, 8, 2, 3, 13))
        + chunk(b"fdAT", struct.pack(">I", 2) + zlib.compress((b"\0" + gray) * 8, 0))
        + chunk(b"IEND", b"")
    )
    (coverage / "motion.apng").write_bytes(ap)
    # A standards-compliant OpenRaster layer stack with stored mimetype first.
    stack = b'<image version="0.0.3" w="24" h="16"><stack><layer name="Test" src="data/layer0.png"/></stack></image>'
    with zipfile.ZipFile(coverage / "layers.ora", "w") as archive:
        for name, body, method in [
            ("mimetype", b"image/openraster", zipfile.ZIP_STORED),
            ("stack.xml", stack, zipfile.ZIP_DEFLATED),
            ("data/layer0.png", sprite, zipfile.ZIP_DEFLATED),
            ("mergedimage.png", sprite, zipfile.ZIP_DEFLATED),
        ]:
            info = zipfile.ZipInfo(name, (2000, 1, 1, 0, 0, 0))
            info.external_attr = 0o100644 << 16
            archive.writestr(info, body, compress_type=method, compresslevel=0)
    rgb = bytes(
        k
        for y in range(h)
        for x in range(w)
        for k in (20, 200 if x // 4 % 2 else 5, 145)
    )
    (coverage / "text.ppm").write_bytes(
        f"P3\n# metadata\n{w} {h}\n255\n".encode()
        + b" ".join(str(v).encode() for v in rgb)
        + b"\n"
    )
    # QOI_RGB literals only, deliberately leaving the run and index encodings unused.
    (coverage / "tile.qoi").write_bytes(
        b"qoif"
        + struct.pack(">IIBB", w, h, 3, 0)
        + b"".join(b"\xfe" + rgb[i : i + 3] for i in range(0, len(rgb), 3))
        + bytes(7)
        + b"\x01"
    )
    (coverage / "strip.tga").write_bytes(
        struct.pack("<BBBHHBHHHHBB", 0, 0, 2, 0, 0, 0, 0, 0, w, h, 24, 0x20)
        + b"".join(rgb[i : i + 3][::-1] for i in range(0, len(rgb), 3))
    )

    return primary, coverage


COVERAGE_FILES = (
    "cursor.cur",
    "icon.ico",
    "layers.ora",
    "metadata-probe.jpg",
    "metadata-probe.png",
    "motion.apng",
    "motion.gif",
    "motion.webp",
    "private-safe.png",
    "private-unsafe.png",
    "rgba16-hidden.png",
    "sprite.webp",
    "strip.tga",
    "strip.tif",
    "stripes.bmp",
    "text.ppm",
    "tile.qoi",
    "trailing.jpg",
    "vector.svg",
    "vector.svgz",
)
