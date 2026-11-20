#!/usr/bin/env python3
"""Measure ECT or Oxipng on fresh copies of PixMinima's pinned primary images."""
import argparse
import csv
import hashlib
import json
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tempfile
import time

BENCH = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BENCH))
import fixtures
from run import compare
from verify import chunks, require_identity, signature


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def command_version(command):
    done = subprocess.run(command, capture_output=True, text=True, check=True, timeout=12)
    return done.stdout.splitlines()


def flags(tool, sources):
    if tool == "ect":
        return ["-9", "--strict", "--mt-file=8", "-progressive"]
    keep = sorted({kind.decode("ascii") for src in sources for kind, _ in chunks(src.read_bytes())
                   if kind[0] & 32 and kind != b"tRNS"})
    args = ["-o", "max", "--zopfli", "--threads", "8"]
    if keep:
        args += ["--keep", ",".join(keep)]
    return args


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tool", choices=["ect", "oxipng"])
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--corpus", type=Path, required=True, help="prepared benchmark directory containing primary/")
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--timeout", type=int, default=1200, help="whole batch timeout in seconds")
    args = parser.parse_args()
    exe = args.binary.resolve()
    manifest = fixtures.MANIFEST["files"]
    selected = [args.corpus / "primary" / name for name in sorted(manifest)
                if args.tool == "ect" or name.endswith(".png")]
    for path in selected:
        if sha(path) != manifest[path.name]:
            raise ValueError(f"Source checksum differs from corpus manifest: {path.name}")
    version = command_version([exe, "-help" if args.tool == "ect" else "--version"])
    version = next(line for line in version if "Version " in line or "oxipng " in line)
    args.out.mkdir(parents=True, exist_ok=True)
    options = flags(args.tool, selected)
    results = []
    with tempfile.TemporaryDirectory(prefix="pixminima-specialist-") as tmp:
        folder = Path(tmp)
        cached = {src.name: (signature(src), require_identity(src)) for src in selected}
        for src in selected:
            shutil.copy2(src, folder / src.name)
        start = time.perf_counter()
        try:
            process = subprocess.run([exe, *options, *(src.name for src in selected)], cwd=folder,
                                     capture_output=True, timeout=args.timeout)
            finished = "complete" if process.returncode == 0 else f"exit-{process.returncode}"
            if process.returncode:
                print(process.stderr.decode(errors="replace")[-2000:], file=sys.stderr)
        except subprocess.TimeoutExpired:
            finished = "timeout"
        seconds = time.perf_counter() - start
        for src in selected:
            proposal = folder / src.name
            status, detail, accept = compare(src, proposal, *cached[src.name])
            original = src.stat().st_size
            raw = proposal.stat().st_size if proposal.is_file() else None
            results.append(dict(file=src.name, original_bytes=original, candidate_bytes=raw,
                                accepted_bytes=min(original, raw) if accept else original,
                                # An incomplete command may have left untouched working copies.
                                status=status, detail=detail, original_sha256=manifest[src.name],
                                candidate_sha256=sha(proposal) if raw is not None else ""))
    with (args.out / f"{args.tool}.csv").open("w", newline="") as out:
        writer = csv.DictWriter(out, fieldnames=list(results[0]))
        writer.writeheader()
        writer.writerows(results)
    summary = dict(tool=args.tool, version=version, binary_sha256=sha(exe), flags=options,
                   completion=finished, files=len(selected), timeout_seconds=args.timeout,
                   elapsed_seconds=round(seconds, 3), machine=platform.machine(),
                   system=platform.system(), macos=platform.mac_ver()[0],
                   source_bytes=sum(row["original_bytes"] for row in results),
                   accepted_bytes=sum(row["accepted_bytes"] for row in results))
    (args.out / f"{args.tool}.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps(summary))
    return 0 if finished == "complete" else 1


if __name__ == "__main__":
    sys.exit(main())
