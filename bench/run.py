#!/usr/bin/env python3
"""Benchmark real commands on fresh copies and verify outputs outside the timer."""

import argparse
import csv
import json
import os
from pathlib import Path
import platform
import plistlib
import random
import shutil
import statistics
import subprocess
import tempfile
import time

import fixtures
from verify import chunks, digest, require_identity, signature

HERE = Path(__file__).resolve().parent
BACKENDS = [
    "pixminima",
    "pixminima-default",
    "pixminima-maximum",
    "chisel",
    "chisel-high",
    "imageoptim-app",
    "imageoptim-engines",
]
PIXMINIMA_FLAGS = {
    "pixminima": ["--thorough"],
    "pixminima-default": [],
    "pixminima-maximum": ["--maximum"],
}


def execute(cmd, env=None):
    r = subprocess.run(
        [str(a) for a in cmd],
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        timeout=3600,
    )
    return r.returncode


def app_command(app, work, inputs):
    if platform.system() != "Darwin":
        raise ValueError("ImageOptim requires macOS")
    fake = work / "home"
    temp = fake / "tmp"
    temp.mkdir(parents=True)
    (fake / "Library/Preferences").mkdir(parents=True)
    (fake / ".Trash").mkdir()
    home = Path.home()
    params = {
        "BENCH": work.resolve(),
        "TEMP": Path(tempfile.gettempdir()).resolve(),
        "PREFS": home / "Library/Preferences",
        "GROUP": home / "Library/Group Containers",
        "CONTAINER": home / "Library/Containers",
    }
    prefix = ["/usr/bin/sandbox-exec"]
    for key, val in params.items():
        prefix += ["-D", key + "=" + str(val)]
    cmd = prefix + [
        "-f",
        HERE / "imageoptim.sb",
        app / "Contents/MacOS/ImageOptim",
        *inputs,
    ]
    return cmd, dict(
        os.environ, HOME=str(fake), CFFIXED_USER_HOME=str(fake), TMPDIR=str(temp)
    )


def engine_candidates(app, source, candidates):
    tools = app / "Contents/Frameworks/ImageOptimGPL.framework/Resources"
    suffix = source.suffix.lower()
    if suffix in (".png", ".apng"):
        try:
            keep = sorted(
                {
                    t.decode("ascii")
                    for t, b in chunks(source.read_bytes())
                    if t[0] & 32 and t != b"tRNS"
                }
            )
        except Exception:
            return []
        oks = ["--keep", ",".join(keep)] if keep else []
        tasks = [
            (
                "oxipng",
                [tools / "oxipng", "-o", "max", "--zopfli", "-q", *oks, "--out"],
                False,
            ),
            (
                "zopflipng",
                [
                    tools / "zopflipng",
                    "-y",
                    *(["--keepchunks=" + ",".join(keep)] if keep else []),
                    source,
                ],
                True,
            ),
        ]
    elif suffix in (".jpg", ".jpeg"):
        tasks = [
            (
                "jpegtran",
                [tools / "jpegtran", "-copy", "all", "-optimize", "-outfile"],
                False,
            ),
            (
                "progressive",
                [
                    tools / "jpegtran",
                    "-copy",
                    "all",
                    "-optimize",
                    "-progressive",
                    "-outfile",
                ],
                False,
            ),
        ]
    elif suffix == ".gif":
        tasks = [("gifsicle", [tools / "gifsicle", "-O3", "--careful", "-o"], False)]
    else:
        return []
    results = []
    for label, prefix, source_already in tasks:
        out = candidates / (source.stem + "-" + label + suffix)
        args = [*prefix, out] if source_already else [*prefix, out, source]
        if execute(args) == 0 and out.is_file():
            results.append(out)
    return results


def write_report(folder, rows, times, config):
    report = {"configuration": config, "files": rows, "runs": times}
    (folder / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    for name, records in [("files", rows), ("runs", times)]:
        if records:
            with (folder / (name + ".csv")).open("w", newline="") as out:
                w = csv.DictWriter(out, fieldnames=list(records[0]))
                w.writeheader()
                w.writerows(records)
    summaries = []
    for suite, backend in sorted({(r["suite"], r["backend"]) for r in times}):
        subset = [r for r in times if (r["suite"], r["backend"]) == (suite, backend)]
        summaries.append(
            {
                "suite": suite,
                "backend": backend,
                "runs": len(subset),
                "median_seconds": round(
                    statistics.median(r["elapsed_seconds"] for r in subset), 6
                ),
                "min_seconds": min(r["elapsed_seconds"] for r in subset),
                "max_seconds": max(r["elapsed_seconds"] for r in subset),
                "input_bytes": subset[0]["input_bytes"],
                "verified_bytes": subset[0]["verified_bytes"],
                "verified_min_bytes": min(r["verified_bytes"] for r in subset),
                "verified_max_bytes": max(r["verified_bytes"] for r in subset),
                "fallback_files": subset[0]["fallback_files"],
            }
        )
    if summaries:
        with (folder / "summary.csv").open("w", newline="") as out:
            w = csv.DictWriter(out, fieldnames=list(summaries[0]))
            w.writeheader()
            w.writerows(summaries)


def compare(source, out, before, sensitive):
    if not out.exists():
        return "missing-output", "", False
    data = out.read_bytes()
    if data == source.read_bytes():
        return "identical", "", True
    if sensitive:
        return "rejected", "requires_byte_identity", False
    try:
        after = signature(out)
    except Exception as e:
        return "unverified", type(e).__name__, False
    keys = sorted(
        k for k in before.keys() | after.keys() if before.get(k) != after.get(k)
    )
    if keys:
        return "rejected", "|".join(keys), False
    if len(data) >= source.stat().st_size:
        return "no-gain", "", True
    return "verified-smaller", "", True


def app_version(app):
    try:
        return plistlib.loads((app / "Contents/Info.plist").read_bytes())[
            "CFBundleShortVersionString"
        ]
    except Exception:
        return None


def version(binary):
    try:
        return (
            subprocess.check_output(
                [str(binary), "--version"], stderr=subprocess.DEVNULL, text=True
            )
            .strip()
            .splitlines()[0]
        )
    except Exception:
        return None


def main():
    parser = argparse.ArgumentParser(__doc__)
    parser.add_argument("--corpus", type=Path, required=True)
    parser.add_argument("--source-dir", type=Path, action="append", default=[])
    parser.add_argument("--prepare-only", action="store_true")
    parser.add_argument("--out", type=Path)
    parser.add_argument("--pixminima", type=Path)
    parser.add_argument("--chisel", type=Path)
    parser.add_argument(
        "--imageoptim", type=Path, default=Path("/Applications/ImageOptim.app")
    )
    parser.add_argument("--backends", nargs="+", choices=BACKENDS, default=BACKENDS)
    parser.add_argument(
        "--suites",
        nargs="+",
        choices=["primary", "coverage", "smoke", "extended"],
        default=["primary", "coverage"],
    )
    parser.add_argument("--repeat", type=int, default=1)
    parser.add_argument("--seed", type=int, default=701)
    parser.add_argument("--no-timer", action="store_true")
    args = parser.parse_args()
    root = args.corpus.resolve()
    fixtures.prepare(root, args.source_dir, "extended" in args.suites)
    if args.prepare_only:
        return
    if args.repeat < 1 or not args.out:
        parser.error(
            "A fresh output directory and a positive repeat count are required"
        )
    for name in ["pixminima", "chisel"]:
        if any(b.split("-")[0] == name for b in args.backends) and (
            not getattr(args, name) or not getattr(args, name).is_file()
        ):
            parser.error("Missing --" + name)
    folder = args.out.resolve()
    folder.mkdir(parents=True, exist_ok=False)
    with tempfile.TemporaryDirectory(dir=folder) as tmp:
        work = Path(tmp)
        binaries = {}
        for name in ["pixminima", "chisel"]:
            if any(b.split("-")[0] == name for b in args.backends):
                binaries[name] = Path(shutil.copy2(getattr(args, name), work / name))
        app = args.imageoptim.resolve()
        config = {
            "platform": platform.system(),
            "release": platform.release(),
            "architecture": platform.machine(),
            "cpus": os.cpu_count(),
            "seed": args.seed,
            "pixminima_mode": "default, --thorough, or --maximum; default compute policy",
            "chisel_mode": "--verify-checksums; default threads; default or 100/50 Zopfli iterations",
            "imageoptim_app_mode": "installed synchronous app; factory defaults; operating-system preferences blocked",
            "imageoptim_engines_mode": "bundled oxipng max/Zopfli, zopflipng or JPEG/GIF tools; per-file verified best",
            "binaries": {
                k: {"sha256": digest(v.read_bytes()), "version": version(v)}
                for k, v in binaries.items()
            },
        }

        def label(command):
            return (
                subprocess.check_output(command, stderr=subprocess.STDOUT, text=True)
                .strip()
                .splitlines()[0]
            )

        config["tools"] = {
            "python": platform.python_version(),
            "ffmpeg": label(["ffmpeg", "-version"]),
            "jpegtran": label(["jpegtran", "-version"]),
            "cwebp": label(["cwebp", "-version"]),
        }
        if platform.system() == "Darwin":
            try:
                config["hardware"] = label(["sysctl", "-n", "machdep.cpu.brand_string"])
            except subprocess.CalledProcessError:
                config["hardware"] = None
            try:
                config["memory_bytes"] = int(label(["sysctl", "-n", "hw.memsize"]))
            except subprocess.CalledProcessError:
                config["memory_bytes"] = None
            config["macos"] = label(["sw_vers", "-productVersion"])
        if any(b.startswith("imageoptim") for b in args.backends):
            config["imageoptim_version"] = app_version(app)
            config["imageoptim_sha256"] = digest(
                (app / "Contents/MacOS/ImageOptim").read_bytes()
            )
            if "imageoptim-app" in args.backends and execute(
                ["codesign", "--verify", "--strict", app]
            ):
                raise RuntimeError("ImageOptim signature did not verify")
        rng = random.Random(args.seed)
        rows = []
        times = []
        for suite in args.suites:
            names = {
                "primary": fixtures.MANIFEST["files"],
                "extended": fixtures.MANIFEST["extended"],
                "coverage": fixtures.COVERAGE_FILES,
                "smoke": ["metadata-probe.png", "metadata-probe.jpg"],
            }[suite]
            selected = [
                root / ("coverage" if suite == "smoke" else suite) / n
                for n in sorted(names)
            ]
            originals = {s.name: (signature(s), require_identity(s)) for s in selected}
            for rep in range(1, args.repeat + 1):
                order = list(args.backends)
                rng.shuffle(order)
                for backend in order:
                    case = work / (suite + "-" + backend + "-" + str(rep))
                    inputs = case / "input"
                    dest = case / "output"
                    inputs.mkdir(parents=True)
                    dest.mkdir()
                    files = [
                        Path(shutil.copyfile(s, inputs / s.name)) for s in selected
                    ]
                    offers = {}
                    ret = 0
                    env = None
                    if backend.startswith("pixminima"):
                        cmd = [
                            binaries["pixminima"],
                            *PIXMINIMA_FLAGS[backend],
                            "--output-dir",
                            dest,
                            inputs,
                        ]
                    elif backend.startswith("chisel"):
                        cmd = [
                            binaries["chisel"],
                            "--quiet",
                            "--verify-checksums",
                            *(
                                ["--iterations", "100", "--iterations-large", "50"]
                                if backend == "chisel-high"
                                else []
                            ),
                            "--output",
                            dest,
                            *files,
                        ]
                    elif backend == "imageoptim-app":
                        cmd, env = app_command(app, case, files)
                    start = time.perf_counter()
                    if backend == "imageoptim-engines":
                        for f in files:
                            offers[f.name] = engine_candidates(app, f, dest)
                    else:
                        ret = execute(cmd, env)
                    elapsed = time.perf_counter() - start
                    sum_raw = 0
                    sum_safe = 0
                    fallback = 0
                    verified_count = 0
                    for src in selected:
                        orig = src.stat().st_size
                        if backend == "imageoptim-app":
                            options = [inputs / src.name]
                        elif backend == "imageoptim-engines":
                            options = offers[src.name]
                        else:
                            options = [dest / src.name]
                        matches = []
                        for output in options:
                            st, detail, ok = compare(src, output, *originals[src.name])
                            raw = output.stat().st_size if output.is_file() else None
                            matches.append((ok, raw, st, detail, output))
                        matches.sort(
                            key=lambda i: (not i[0], i[1] if i[1] is not None else orig)
                        )
                        ok, raw, status, detail, candidate = (
                            matches[0]
                            if matches
                            else (False, None, "no-engine", "", None)
                        )
                        if ret and status == "missing-output":
                            detail = ("exit=" + str(ret) + " " + detail).strip()
                        safe = min(orig, raw) if ok else orig
                        if not ok:
                            fallback += 1
                        if safe < orig:
                            verified_count += 1
                        sum_raw += raw if raw is not None else orig
                        sum_safe += safe
                        rows.append(
                            dict(
                                suite=suite,
                                repetition=rep,
                                backend=backend,
                                file=src.name,
                                original_bytes=orig,
                                candidate_bytes=raw,
                                verified_bytes=safe,
                                saved=orig - safe,
                                status=status,
                                detail=detail,
                                original_sha256=digest(src.read_bytes()),
                                candidate_sha256=digest(candidate.read_bytes())
                                if candidate and candidate.is_file()
                                else "",
                            )
                        )
                    times.append(
                        dict(
                            suite=suite,
                            repetition=rep,
                            backend=backend,
                            elapsed_seconds=round(elapsed, 6)
                            if not args.no_timer
                            else 0,
                            input_bytes=sum(s.stat().st_size for s in selected),
                            candidate_bytes=sum_raw,
                            verified_bytes=sum_safe,
                            verified_smaller_files=verified_count,
                            fallback_files=fallback,
                            process_status=ret,
                        )
                    )
                    write_report(folder, rows, times, config)
                    print(suite, rep, backend, times[-1], flush=True)


if __name__ == "__main__":
    main()
