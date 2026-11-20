"""Independent losslessness checks for the benchmark. No PixMinima codec is imported."""

from array import array
import gzip
import hashlib
import struct
import subprocess
import sys
import zlib
from xml.etree import ElementTree as ET


def digest(data):
    return hashlib.sha256(data).hexdigest()


def native(args):
    p = subprocess.run(args, capture_output=True, timeout=120)
    if p.returncode:
        raise ValueError(f"{args[0]} could not decode the image")
    return p.stdout


def chunks(data):
    if not data.startswith(b"\x89PNG\r\n\x1a\n"):
        raise ValueError("PNG signature")
    p = 8
    while p < len(data):
        n, t = struct.unpack_from(">I4s", data, p)
        body = data[p + 8 : p + 8 + n]
        if (zlib.crc32(t + body) & 0xFFFFFFFF) != struct.unpack_from(
            ">I", data, p + 8 + n
        )[0]:
            raise ValueError("PNG CRC")
        p += n + 12
        yield t, body
        if t == b"IEND":
            if p != len(data):
                yield b"TAIL", data[p:]
            return
    raise ValueError("Incomplete PNG")


def pairs(w, h, interlace):
    if not interlace:
        yield 0, 0, 1, 1, w, h
        return
    for x, y, dx, dy in [
        (0, 0, 8, 8),
        (4, 0, 8, 8),
        (0, 4, 4, 8),
        (2, 0, 4, 4),
        (0, 2, 2, 4),
        (1, 0, 2, 2),
        (0, 1, 1, 2),
    ]:
        pw, ph = max(0, (w - x + dx - 1) // dx), max(0, (h - y + dy - 1) // dy)
        if pw and ph:
            yield x, y, dx, dy, pw, ph


def unfilter(raw, previous, bpp, method):
    if method == 0:
        return raw
    if method == 2:
        return bytearray((a + b) & 255 for a, b in zip(raw, previous))
    for i in range(len(raw)):
        a = raw[i - bpp] if i >= bpp else 0
        b = previous[i]
        c = previous[i - bpp] if i >= bpp else 0
        if method == 1:
            prediction = a
        elif method == 3:
            prediction = (a + b) // 2
        elif method == 4:
            p = a + b - c
            da, db, dc = abs(p - a), abs(p - b), abs(p - c)
            prediction = a if da <= db and da <= dc else b if db <= dc else c
        else:
            raise ValueError("PNG filter")
        raw[i] = (raw[i] + prediction) & 255
    return raw


def samples(row, depth, count):
    if depth == 8:
        return row
    if depth == 16:
        return struct.unpack(">" + "H" * count, row)
    mask = (1 << depth) - 1
    return [
        (row[(i * depth) // 8] >> (8 - depth - i * depth % 8)) & mask
        for i in range(count)
    ]


def png_pixels(fields):
    header = next(b for t, b in fields if t == b"IHDR")
    w, h, d, c, comp, filt, interlace = struct.unpack(">IIBBBBB", header)
    if w * h > 20_000_000 or comp or filt or interlace not in [0, 1]:
        raise ValueError("Unsupported PNG")
    channels = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}[c]
    pl = next((b for t, b in fields if t == b"PLTE"), b"")
    tr = next((b for t, b in fields if t == b"tRNS"), b"")
    transparent = (
        tuple(n for (n,) in struct.iter_unpack(">H", tr)) if tr and c in (0, 2) else ()
    )
    scale = 65535 // ((1 << d) - 1)
    # The canonical bytes represent unassociated RGBA, including RGB at zero alpha.
    pixels = array("H", [0]) * (w * h * 4)
    filtered = zlib.decompress(b"".join(b for t, b in fields if t == b"IDAT"))
    pos = 0
    for x, y, dx, dy, pw, ph in pairs(w, h, interlace):
        stride = (pw * d * channels + 7) // 8
        previous = bytearray(stride)
        for py in range(ph):
            row = bytearray(filtered[pos + 1 : pos + 1 + stride])
            if len(row) != stride:
                raise ValueError("PNG row length")
            previous = unfilter(
                row, previous, max(1, (d * channels + 7) // 8), filtered[pos]
            )
            pos += stride + 1
            vals = samples(previous, d, pw * channels)
            converted = array("H")
            for k in range(0, pw * channels, channels):
                v = vals[k : k + channels]
                if c == 3:
                    j = v[0]
                    rgb = pl[j * 3 : j * 3 + 3]
                    if len(rgb) != 3:
                        raise ValueError("palette index")
                    converted.extend(x * 257 for x in rgb)
                    converted.append((tr[j] if j < len(tr) else 255) * 257)
                elif c in (0, 4):
                    g = v[0] * scale
                    a = (
                        v[1] * scale
                        if c == 4
                        else (0 if (v[0],) == transparent else 65535)
                    )
                    converted.extend((g, g, g, a))
                else:
                    converted.extend(value * scale for value in v[:3])
                    converted.append(
                        v[3] * scale
                        if c == 6
                        else (0 if tuple(v) == transparent else 65535)
                    )
            start = ((y + py * dy) * w + x) * 4
            if dx == 1:
                pixels[start : start + len(converted)] = converted
            else:
                for k in range(pw):
                    a = start + k * dx * 4
                    pixels[a : a + 4] = converted[k * 4 : k * 4 + 4]
    if pos != len(filtered):
        raise ValueError("Unexpected PNG samples")
    if sys.byteorder == "little":
        pixels.byteswap()
    return digest(pixels.tobytes())


def apng_frames(fields):
    animation = next((b for t, b in fields if t == b"acTL"), None)
    if animation is None:
        return None
    header = fields[0][1]
    global_chunks = [(t, b) for t, b in fields if t in (b"PLTE", b"tRNS")]
    frames = []
    control = None
    parts = []
    sequence = 0

    def finish():
        if control is not None:
            pixels = png_pixels(
                [
                    (b"IHDR", control[:8] + header[8:]),
                    *global_chunks,
                    (b"IDAT", b"".join(parts)),
                ]
            )
            frames.append((control, pixels))

    for t, b in fields:
        if t in (b"fcTL", b"fdAT"):
            if int.from_bytes(b[:4], "big") != sequence:
                raise ValueError("Invalid APNG sequence")
            sequence += 1
        if t == b"fcTL":
            finish()
            control = b[4:]
            parts = []
        elif t == b"fdAT":
            parts.append(b[4:])
        elif t == b"IDAT" and control is not None:
            parts.append(b)
    finish()
    if len(frames) != int.from_bytes(animation[:4], "big"):
        raise ValueError("APNG frame count")
    return animation, tuple(frames)


def png_semantics(data):
    f = list(chunks(data))
    w, h, d, c, *_ = struct.unpack(">IIBBBBB", f[0][1])
    palette = next((b for t, b in f if t == b"PLTE"), b"")
    meta = []
    for t, b in f:
        if t in (b"IHDR", b"IDAT", b"IEND", b"tRNS", b"acTL", b"fcTL", b"fdAT"):
            continue
        if t == b"PLTE" and c == 3:
            continue
        if t == b"iCCP":
            name, body = b.split(b"\0", 1)
            if body[:1] != b"\0":
                raise ValueError("iCCP compression")
            b = name + b"\0" + zlib.decompress(body[1:])
        elif t == b"zTXt":
            name, body = b.split(b"\0", 1)
            b = name + b"\0" + zlib.decompress(body[1:])
            t = b"tEXt"
        elif t == b"iTXt":
            key, rest = b.split(b"\0", 1)
            lang, translated, text = rest[2:].split(b"\0", 2)
            if rest[0]:
                text = zlib.decompress(text)
            b = b"\0".join([key, lang, translated, text])
        elif t == b"bKGD":
            if c == 3:
                b = bytes(palette[3 * b[0] : 3 * b[0] + 3])
                b = struct.pack(">3H", *(x * 257 for x in b))
            else:
                v = tuple(
                    i * (65535 // ((1 << d) - 1))
                    for (i,) in struct.iter_unpack(">H", b)
                )
                b = struct.pack(">3H", *(v * 3 if len(v) == 1 else v))
        elif t == b"sBIT":
            if c in (0, 4):
                b = b[:1] * 3 + b[1:]
            if len(b) == 3:
                b += bytes([16])  # no per-pixel alpha channel
        meta.append((t, digest(b)))
    return {
        "shape": (w, h),
        "pixels_rgba16": png_pixels(f),
        "metadata": tuple(meta),
        "animation": apng_frames(f),
    }


def jpeg_info(data):
    pos = 2
    inside = False
    meta = []
    if data[:2] != b"\xff\xd8":
        raise ValueError("JPEG signature")
    while pos < len(data):
        if inside:
            pos = data.index(255, pos)
        if data[pos] != 255:
            raise ValueError("JPEG marker")
        while data[pos] == 255:
            pos += 1
        t = data[pos]
        pos += 1
        if inside and (t == 0 or 208 <= t <= 215):
            continue
        inside = False
        if t == 217:
            if pos < len(data):
                meta.append(("tail", digest(data[pos:])))
            return tuple(meta)
        size = int.from_bytes(data[pos : pos + 2], "big")
        if size < 2 or pos + size > len(data):
            raise ValueError("JPEG segment size")
        b = data[pos + 2 : pos + size]
        pos += size
        if 224 <= t <= 239 or t == 254:
            meta.append((t, digest(b)))
        if t == 218:
            inside = True
    raise ValueError("Incomplete JPEG")


def ffmpeg_frames(path, native_eight=False, raw_eight=False):
    # FFmpeg's direct conversion of native 8-bit palette/BGR to 16-bit can pass
    # through different scaler matrices. Expand each exact RGBA8 channel by 257.
    pixel_format = "rgba" if native_eight or raw_eight else "rgba64be"
    raw = native(
        [
            "ffmpeg",
            "-v",
            "error",
            "-noautorotate",
            "-i",
            str(path),
            "-map",
            "0:v:0",
            "-fps_mode",
            "passthrough",
            "-pix_fmt",
            pixel_format,
            "-f",
            "rawvideo",
            "-",
        ]
    )
    if not raw:
        raise ValueError("No decoded frames")
    if raw_eight:
        return raw
    if native_eight:
        expanded = bytearray(len(raw) * 2)
        expanded[::2] = raw
        expanded[1::2] = raw
        return digest(expanded)
    return digest(raw)


def jpg_semantics(path, data):
    # libjpeg-turbo canonicalizes the quantized DCT coefficients, quantizer tables,
    # dimensions, component sampling and entropy coding under a fixed scan script.
    canonical = native(
        ["jpegtran", "-copy", "none", "-progressive", "-optimize", str(path)]
    )
    return {
        "jpeg_dct": digest(canonical),
        "metadata": jpeg_info(data),
        "pixels_rgba16": ffmpeg_frames(path),
    }


def riff_chunks(data):
    p = 0
    while p < len(data):
        t, n = struct.unpack_from("<4sI", data, p)
        b = data[p + 8 : p + 8 + n]
        if len(b) != n:
            raise ValueError("RIFF chunk length")
        yield t, b
        p += 8 + n + (n & 1)
    if p != len(data):
        raise ValueError("RIFF padding")


def webp_semantics(data, path, raw=False):
    if (
        data[:4] != b"RIFF"
        or int.from_bytes(data[4:8], "little") + 8 != len(data)
        or data[8:12] != b"WEBP"
    ):
        raise ValueError("WebP container")
    meta = []
    frames = []
    for t, b in riff_chunks(data[12:]):
        if t in (b"VP8 ", b"VP8L", b"ALPH"):
            continue
        if t == b"ANMF":
            # Decode every full frame before the canvas applies its blend/disposal.
            parts = list(riff_chunks(b[16:]))
            if any(k not in (b"VP8L", b"VP8 ", b"ALPH") for k, v in parts):
                raise ValueError("Unknown ANMF chunk")
            import tempfile
            from pathlib import Path

            with tempfile.TemporaryDirectory() as tmp:
                container = b"WEBP" + b[16:]
                f = Path(tmp) / "frame.webp"
                f.write_bytes(b"RIFF" + struct.pack("<I", len(container)) + container)
                frames.append(
                    (b[:16], ffmpeg_frames(f, native_eight=True, raw_eight=raw))
                )
        else:
            meta.append((t, digest(b)))
    return {
        "metadata": tuple(meta),
        "pixels_rgba16": tuple(frames)
        if frames
        else ffmpeg_frames(path, native_eight=True, raw_eight=raw),
    }


def subblocks(data, p):
    chunks = []
    while data[p]:
        n = data[p]
        p += 1
        chunks.append(data[p : p + n])
        p += n
    return b"".join(chunks), p + 1


def gif_indices(payload, minimum, count):
    clear = 1 << minimum
    end = clear + 1
    indices = bytearray()
    bit = 0
    table = []
    previous = b""
    while bit < len(payload) * 8:
        if not table:
            table = [bytes([k]) for k in range(clear)] + [b"", b""]
            width = minimum + 1
            previous = b""
        code = (
            int.from_bytes(payload[bit // 8 : bit // 8 + 3], "little") >> (bit % 8)
        ) & ((1 << width) - 1)
        bit += width
        if code == clear:
            table = []
            continue
        if code == end:
            break
        if code < len(table):
            entry = table[code]
        elif code == len(table) and previous:
            entry = previous + previous[:1]
        else:
            raise ValueError("GIF LZW")
        indices.extend(entry)
        if len(indices) > count:
            raise ValueError("GIF dimensions")
        if previous and len(table) < 4096:
            table.append(previous + entry[:1])
            if len(table) == 1 << width and width < 12:
                width += 1
        previous = entry
    if len(indices) != count:
        raise ValueError("GIF sample count")
    return indices


def gif_semantics(data, path, raw=False):
    w, h, flags, bg, aspect = struct.unpack_from("<HHBBB", data, 6)
    p = 13
    pal = b""
    trans = None
    frames = []
    meta = []
    palettes = []
    if flags & 128:
        n = 3 * (2 << (flags & 7))
        pal = data[p : p + n]
        if raw:
            palettes.append(("global", pal))
        p += n
    while p < len(data):
        t = data[p]
        p += 1
        if t == 0x3B:
            if p < len(data):
                meta.append(("tail", digest(data[p:])))
            break
        if t == 0x21:
            t = data[p]
            b, p = subblocks(data, p + 1)
            if t == 249:
                # Transparent indices may be remapped with their palette.
                trans = b[3] if b[0] & 1 else None
                meta.append((t, b[:3], len(frames)))
            else:
                meta.append((t, b, len(frames)))
        elif t == 0x2C:
            x, y, fw, fh, fl = struct.unpack_from("<HHHHB", data, p)
            p += 9
            active = pal
            if fl & 128:
                n = 3 * (2 << (fl & 7))
                active = data[p : p + n]
                if raw:
                    palettes.append((len(frames), active))
                p += n
            depth = data[p]
            packed, p = subblocks(data, p + 1)
            indices = gif_indices(packed, depth, fw * fh)
            rows = (
                list(range(fh))
                if not fl & 64
                else list(range(0, fh, 8))
                + list(range(4, fh, 8))
                + list(range(2, fh, 4))
                + list(range(1, fh, 2))
            )
            rgba = bytearray(fw * fh * 4)
            for sy, dy in enumerate(rows):
                for k, index in enumerate(indices[sy * fw : (sy + 1) * fw]):
                    j = (dy * fw + k) * 4
                    rgba[j : j + 4] = active[index * 3 : index * 3 + 3] + bytes(
                        [0 if index == trans else 255]
                    )
            frames.append((x, y, fw, fh, bytes(rgba) if raw else digest(rgba)))
            trans = None
        else:
            raise ValueError("Unknown GIF block")
    return {
        "shape": (w, h, aspect),
        "background": pal[bg * 3 : bg * 3 + 3],
        "metadata": tuple(meta),
        "raw_frames": tuple(frames),
        "pixels_rgba16": ffmpeg_frames(path, native_eight=True),
        **({"palettes": palettes} if raw else {}),
    }


def tiff_info(data):
    order = {b"II": "<", b"MM": ">"}[data[:2]]

    def unpack(fmt, buf):
        return struct.unpack(order + fmt, buf)

    if unpack("H", data[2:4])[0] != 42:
        raise ValueError("TIFF type")
    off = unpack("I", data[4:8])[0]
    seen = set()
    pages = []
    directories = []
    sizes = {1: 1, 2: 1, 3: 2, 4: 4, 5: 8, 6: 1, 7: 1, 8: 2, 9: 4, 10: 8, 11: 4, 12: 8}
    while off:
        if off in seen:
            raise ValueError("TIFF circular directories")
        seen.add(off)
        n = unpack("H", data[off : off + 2])[0]
        p = off + 2
        meta = []
        for i in range(n):
            tag, ty, count = unpack("HHI", data[p : p + 8])
            value = data[p + 8 : p + 12]
            p += 12
            size = sizes[ty] * count
            if size > 4:
                a = unpack("I", value)[0]
                value = data[a : a + size]
            else:
                value = value[:size]
            if tag not in (259, 273, 278, 279, 317):
                meta.append((tag, ty, count, digest(value)))
        directories.append((off, p))
        pages.append(tuple(sorted(meta)))
        off = unpack("I", data[p : p + 4])[0]
    return tuple(pages), directories, order


def tiff_semantics(data, path):
    import tempfile
    from pathlib import Path

    pages, directories, order = tiff_info(data)
    rasters = []
    with tempfile.TemporaryDirectory() as tmp:
        page = Path(tmp) / "page.tif"
        for offset, next_at in directories:
            # Point an independent decoder to each complete page, preserving
            # every strip/value offset relative to the original TIFF header.
            standalone = bytearray(data)
            struct.pack_into(order + "I", standalone, 4, offset)
            struct.pack_into(order + "I", standalone, next_at, 0)
            page.write_bytes(standalone)
            rasters.append(ffmpeg_frames(page))
    return {"metadata": pages, "pixels_rgba16": tuple(rasters)}


def bmp_info(data):
    if data[:2] != b"BM" or len(data) < 54:
        raise ValueError("BMP signature")
    # This corpus uses BITMAPINFOHEADER with no extra profiles or application data.
    if int.from_bytes(data[14:18], "little") != 40:
        raise ValueError("Unverified BMP variant")
    return data[6:10], data[38:46]


def svg_xml(data):
    # Keep text (including SVG <text> whitespace), namespace names, attribute
    # values and element order. Preserve comments and processing instructions.
    parser = ET.XMLParser(target=ET.TreeBuilder(insert_comments=True, insert_pis=True))

    def node(e):
        return (
            str(e.tag) if isinstance(e.tag, str) else e.tag.__name__,
            tuple(sorted(e.attrib.items())),
            e.text or "",
            tuple((node(c), c.tail or "") for c in e),
        )

    return node(ET.fromstring(data, parser=parser))


def archive_info(path):
    import zipfile

    with zipfile.ZipFile(path) as archive:
        result = []
        for entry in archive.infolist():
            data = archive.read(entry)
            if (
                entry.filename.lower().endswith(".png")
                and data.startswith(b"\x89PNG")
                and not requires_identity_data(data)
            ):
                content = png_semantics(data)
            else:
                content = digest(data)
            result.append(
                (
                    entry.filename,
                    entry.date_time,
                    entry.extra,
                    # The deflate level bits and data-descriptor switch describe
                    # ZIP encoding. The remaining flags include name encoding.
                    entry.flag_bits & ~0x0E,
                    entry.comment,
                    entry.external_attr,
                    entry.internal_attr,
                    entry.compress_type if entry.filename == "mimetype" else None,
                    content,
                )
            )
        return archive.comment, tuple(result)


def netpbm_header(data):
    import re

    pos = 2
    tokens = []
    comments = []
    for match in re.finditer(rb"#[^\r\n]*|[^\s#]+", data[2:]):
        token = match[0]
        if token.startswith(b"#"):
            comments.append(token)
            continue
        tokens.append(int(token))
        pos = 2 + match.end()
        if len(tokens) == 3:
            break
    if data[1] == 51:
        comments.extend(re.findall(rb"#[^\r\n]*", data[pos:]))
    return tokens, comments


def signature(path):
    data = path.read_bytes()
    if data.startswith(b"\x89PNG"):
        return png_semantics(data)
    if data.startswith(b"\xff\xd8"):
        return jpg_semantics(path, data)
    if data.startswith((b"GIF87a", b"GIF89a")):
        return gif_semantics(data, path)
    if data[8:12] == b"WEBP":
        return webp_semantics(data, path)
    if data[:2] in (b"II", b"MM"):
        return tiff_semantics(data, path)
    if data.startswith(b"BM"):
        return {
            "metadata": bmp_info(data),
            "pixels_rgba16": ffmpeg_frames(path, native_eight=True),
        }
    if path.suffix.lower() in (".ico", ".cur") and data[:4] in (
        b"\0\0\1\0",
        b"\0\0\2\0",
    ):
        count = int.from_bytes(data[4:6], "little")
        entries = []
        for i in range(count):
            p = 6 + 16 * i
            size, offset = struct.unpack_from("<II", data, p + 8)
            member = data[offset : offset + size]
            if not member.startswith(b"\x89PNG"):
                raise ValueError("Only PNG-backed ICO/CUR covered")
            entries.append((data[p : p + 8], png_semantics(member)))
        return {"members": entries}
    if path.suffix.lower() in (".svg", ".svgz"):
        return {
            "xml_tree": svg_xml(
                gzip.decompress(data) if data[:2] == b"\x1f\x8b" else data
            )
        }
    if path.suffix.lower() == ".ora":
        return {"members": archive_info(path)}
    if data[:4] == b"qoif":
        return {
            "shape_and_colorspace": data[4:14],
            "pixels_rgba16": ffmpeg_frames(path, native_eight=True),
        }
    if path.suffix.lower() == ".tga":
        return {
            "metadata": data[:2] + data[3:18] + data[18 : 18 + data[0]],
            "pixels_rgba16": ffmpeg_frames(path, native_eight=True),
        }
    if data[:2] in (b"P3", b"P6"):
        header, comments = netpbm_header(data)
        return {
            "metadata": (header, comments),
            "pixels_rgba16": ffmpeg_frames(path, native_eight=header[2] <= 255),
        }
    raise ValueError("No independent format verifier")


def require_identity(path):
    return requires_identity_data(path.read_bytes())


def requires_identity_data(data):
    if data.startswith(b"\x89PNG"):
        # Unknown unsafe-to-copy chunks may refer to the compressed representation.
        known = {
            b"IHDR",
            b"PLTE",
            b"IDAT",
            b"IEND",
            b"tRNS",
            b"cHRM",
            b"gAMA",
            b"iCCP",
            b"sBIT",
            b"sRGB",
            b"cICP",
            b"mDCV",
            b"cLLI",
            b"bKGD",
            b"hIST",
            b"pHYs",
            b"sPLT",
            b"tIME",
            b"iTXt",
            b"tEXt",
            b"zTXt",
            b"eXIf",
            b"acTL",
            b"fcTL",
            b"fdAT",
        }
        return any(
            t == b"TAIL" or (t not in known and not (t[3] & 32))
            for t, b in chunks(data)
        )
    if data.startswith(b"\xff\xd8"):
        return any(t in ("tail", 235) for t, h in jpeg_info(data)) or b"MPF\0" in data
    return False
