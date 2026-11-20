#!/usr/bin/env python3
"""Compare complete, in-place optimization of distinct, hash-pinned images on a Metal Mac."""
import argparse
import csv
import hashlib
import json
import shutil
import struct
import subprocess
import tempfile
import time
import urllib.request
import zlib
from pathlib import Path

HERE = Path(__file__).resolve().parent
MODES = ("cpu", "metal", "auto")


def digest(data):
    return hashlib.sha256(data).hexdigest()


def chunk(kind, body):
    return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body))


def png(width, height, raw):
    stride = width * 4
    if len(raw) != stride * height:
        raise ValueError("Incorrect RGBA8 dimensions")
    rows = b"".join(b"\0" + raw[i:i + stride] for i in range(0, len(raw), stride))
    header = struct.pack(">II5B", width, height, 8, 6, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) +
            chunk(b"IDAT", zlib.compress(rows, 0)) + chunk(b"IEND", b""))


def prepare(directory):
    corpus = json.loads((HERE / "corpus.json").read_text())
    directory.mkdir(parents=True, exist_ok=True)
    paths = []
    for photo in corpus:
        name = photo["name"]
        path = directory / (name + ".png")
        if not path.exists() or digest(path.read_bytes()) != photo["png_sha256"]:
            with urllib.request.urlopen(photo["url"], timeout=90) as source:
                data = source.read()
            if digest(data) != photo["source_sha256"]:
                raise ValueError("Upstream content changed: " + name)
            command = ["ffmpeg", "-v", "error", "-threads", "1", "-i", "pipe:0",
                       "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"]
            raw = subprocess.run(command, input=data, stdout=subprocess.PIPE, check=True).stdout
            if digest(raw) != photo["rgba_sha256"]:
                raise ValueError("Decoded pixels differ: " + name)
            image = png(photo["width"], photo["height"], raw)
            if digest(image) != photo["png_sha256"]:
                raise ValueError("Prepared image differs: " + name)
            path.write_bytes(image)
        paths.append(path)
    return paths


def complete_batch(binary, directory, mode, files):
    # Copy unmodified inputs outside the timer. Each measured process runs the
    # same permanent replacement workflow as the app, on its own disposable copies.
    with tempfile.TemporaryDirectory(prefix="pixminima-timed-", dir=directory) as temp:
        copies = [Path(shutil.copy2(image, temp)) for image in files]
        start = time.perf_counter()
        run = subprocess.run([binary, "--overwrite", "--json", "--compute", mode, *copies],
                             stdout=subprocess.PIPE, check=True)
        elapsed = time.perf_counter() - start
        reports = [json.loads(line) for line in run.stdout.splitlines()]
        rows = {}
        for report in reports:
            path = Path(report["input"])
            data = path.read_bytes()
            if len(data) != report["after"] or len(data) > report["before"]:
                raise ValueError("Reported size does not match output: " + path.name)
            rows[path.name] = (report["before"], len(data), digest(data))
        if set(rows) != {p.name for p in files}:
            raise ValueError("Incomplete batch")
        return elapsed, rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--write", type=Path, required=True)
    parser.add_argument("--hashes", type=Path, required=True)
    args = parser.parse_args()
    if args.runs < 1:
        parser.error("--runs must be positive")
    if args.write.exists() or args.hashes.exists():
        parser.error("Use fresh report paths")
    binary = args.binary.resolve(strict=True)
    inputs = prepare(args.directory)
    sets = [("mixed", inputs), ("large", [p for p in inputs if p.stat().st_size > 16 << 20])]
    expected = {}
    with args.write.open("x", newline="") as out:
        writer = csv.writer(out, lineterminator="\n")
        writer.writerow(["set", "run", "mode", "seconds", "files", "input_bytes", "output_bytes"])
        for label, files in sets:
            for index in range(args.runs):
                offset = index % len(MODES)
                for mode in MODES[offset:] + MODES[:offset]:
                    elapsed, rows = complete_batch(binary, args.directory, mode, files)
                    for name, record in rows.items():
                        if expected.setdefault(name, record) != record:
                            raise ValueError(f"Output differs for {name} in {label} / {mode}")
                    writer.writerow([label, index + 1, mode, f"{elapsed:.6f}", len(rows),
                                     sum(r[0] for r in rows.values()), sum(r[1] for r in rows.values())])
                    out.flush()
    with args.hashes.open("x", newline="") as out:
        writer = csv.writer(out, lineterminator="\n")
        writer.writerow(["file", "input_bytes", "output_bytes", "output_sha256"])
        for name, record in sorted(expected.items()):
            writer.writerow([name, *record])


if __name__ == "__main__":
    main()
