#!/usr/bin/env python3
"""Check public GIF, WebP and TIFF files and their filesystem attributes after permanent replacement on private copies."""

import argparse
import json
import os
from pathlib import Path
import platform
import plistlib
import shutil
import stat
import subprocess
import tempfile
import urllib.request
import zlib

from verify import chunks, digest, signature

HERE = Path(__file__).resolve().parent
ATTRIBUTES = {
    "com.pixminima.fidelity": b"Opaque audit attribute",
    "com.apple.metadata:_kMDItemUserTags": plistlib.dumps(
        ["Image audit\n2"], fmt=plistlib.FMT_BINARY
    ),
    "com.apple.ResourceFork": b"Audit resource fork",
}


def command(*args):
    return subprocess.run(list(map(str, args)), capture_output=True, check=True).stdout


def pinned(record, folders, destination):
    local = next(
        (p / record["name"] for p in folders if (p / record["name"]).is_file()), None
    )
    if local:
        data = local.read_bytes()
    else:
        with urllib.request.urlopen(record["url"], timeout=45) as response:
            data = response.read()
    if digest(data) != record["sha256"]:
        raise ValueError("Source hash mismatch: " + record["name"])
    destination.write_bytes(data)


def derive(sources, scratch, input_dirs):
    primary = json.loads((HERE / "corpus.json").read_text())
    name = "astronaut.png"
    image = scratch / name
    pinned(
        {"name": name, "url": primary["base"] + name, "sha256": primary["files"][name]},
        input_dirs,
        image,
    )
    iccp = next(body for kind, body in chunks(image.read_bytes()) if kind == b"iCCP")
    profile = scratch / "photo.icc"
    profile.write_bytes(zlib.decompress(iccp.split(b"\0", 1)[1][1:]))
    exif = scratch / "orientation.exif"
    xmp = scratch / "metadata.xmp"
    xmp.write_bytes(
        b'<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"/></x:xmpmeta>'
    )
    photo = sources / "hopper_orientation_6.webp"
    command("dwebp", photo, "-o", scratch / "photo.png")
    command("webpmux", "-get", "exif", photo, "-o", exif)
    command(
        "cwebp",
        "-lossless",
        "-m",
        "0",
        "-exact",
        scratch / "photo.png",
        "-o",
        scratch / "photo.webp",
    )
    command(
        "webpmux",
        "-set",
        "exif",
        exif,
        scratch / "photo.webp",
        "-o",
        scratch / "oriented.webp",
    )
    command(
        "webpmux",
        "-set",
        "icc",
        profile,
        scratch / "oriented.webp",
        "-o",
        scratch / "profiled.webp",
    )
    command(
        "webpmux",
        "-set",
        "xmp",
        xmp,
        scratch / "profiled.webp",
        "-o",
        sources / "derived-photo-lossless-icc-exif-xmp.webp",
    )
    command("dwebp", sources / "transparent.webp", "-o", scratch / "alpha.png")
    command(
        "cwebp",
        "-lossless",
        "-m",
        "0",
        "-exact",
        scratch / "alpha.png",
        "-o",
        sources / "derived-transparent-exact.webp",
    )
    command(
        "gif2webp",
        "-m",
        "0",
        sources / "different_transparency.gif",
        "-o",
        sources / "derived-animation.webp",
    )
    tiff = scratch / "oriented.tif"
    shutil.copyfile(sources / "rgb-3c-16b.tiff", tiff)
    command("tiffset", "-s", "274", "6", tiff)
    command("tiffcp", "-c", "none", tiff, sources / "derived-16bit-orientation.tif")


def set_attributes(path):
    os.chmod(path, 0o640)
    for key, value in ATTRIBUTES.items():
        command("xattr", "-wx", key, value.hex(), path)
    command("chmod", "+a", "everyone allow readattr,readextattr", path)
    command("chflags", "hidden", path)
    command("SetFile", "-d", "01/02/2019 09:10:11", path)
    os.utime(path, ns=(1609459200000000000, 1609459200123456789))


def attributes(path):
    info = path.stat()
    state = {
        "mode": stat.S_IMODE(info.st_mode),
        "owner": (info.st_uid, info.st_gid),
        "modified_time_ns": info.st_mtime_ns,
        "creation_time_seconds": info.st_birthtime,
        "BSD_flags": info.st_flags,
        "ACL": command("ls", "-lde", path).splitlines()[1:],
    }
    for key in ATTRIBUTES:
        state[key] = command("xattr", "-px", key, path).split()
    return state


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pixminima", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--source-dir", type=Path, action="append", default=[])
    parser.add_argument("--mac-attributes", action="store_true")
    parser.add_argument(
        "--effort", choices=("standard", "thorough", "maximum"), default="standard"
    )
    args = parser.parse_args()
    if args.mac_attributes and platform.system() != "Darwin":
        parser.error("Filesystem attribute checks require macOS")
    if args.out.exists():
        parser.error("Use a fresh output file")
    args.out.parent.mkdir(parents=True, exist_ok=True)
    records = json.loads((HERE / "fidelity-sources.json").read_text())
    result = []
    with tempfile.TemporaryDirectory(dir=args.out.parent) as temporary:
        scratch = Path(temporary)
        sources = scratch / "sources"
        copies = scratch / "working-copies"
        sources.mkdir()
        copies.mkdir()
        for record in records:
            pinned(record, args.source_dir, sources / record["name"])
        derive(sources, scratch, args.source_dir)
        for source in sorted(sources.iterdir()):
            candidate = copies / source.name
            shutil.copyfile(source, candidate)
            if args.mac_attributes:
                set_attributes(candidate)
            before = attributes(candidate) if args.mac_attributes else {}
            flags = [] if args.effort == "standard" else ["--" + args.effort]
            run = subprocess.run(
                [args.pixminima.resolve(), *flags, "--overwrite", candidate],
                capture_output=True,
            )
            after = attributes(candidate) if args.mac_attributes else {}
            changed = [key for key in before if before[key] != after[key]]
            source_hash, out_hash = (
                digest(path.read_bytes()) for path in (source, candidate)
            )
            identical = source_hash == out_hash
            valid = identical or signature(source) == signature(candidate)
            result.append(
                dict(
                    file=source.name,
                    input_sha256=source_hash,
                    output_sha256=out_hash,
                    input_bytes=source.stat().st_size,
                    output_bytes=candidate.stat().st_size,
                    result="byte-identical"
                    if identical
                    else "verified"
                    if valid
                    else "mismatch",
                    exit_code=run.returncode,
                    changed_attributes=changed,
                )
            )
    report = dict(
        pixminima_sha256=digest(args.pixminima.read_bytes()),
        effort=args.effort,
        mac_attributes=args.mac_attributes,
        macos=platform.mac_ver()[0],
        files=result,
    )
    args.out.write_text(json.dumps(report, indent=2) + "\n")
    if any(
        r["exit_code"]
        or r["result"] == "mismatch"
        or r["changed_attributes"]
        or r["output_bytes"] > r["input_bytes"]
        for r in result
    ):
        raise SystemExit("A file or attribute check failed. See report.")


if __name__ == "__main__":
    main()
