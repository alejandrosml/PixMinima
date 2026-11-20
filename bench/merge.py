#!/usr/bin/env python3
"""Combine benchmark sessions on the same machine and input data, keeping provenance."""

import argparse
import json
from pathlib import Path

from run import write_report


def merge(sources, destination, replace, note):
    files = []
    runs = []
    sessions = []
    recorded = set()
    replaced = set()
    machine = None
    corpus = None
    for number, source in enumerate(sources, 1):
        report = json.loads((source / "results.json").read_text())
        config = report["configuration"]
        session_machine = {
            key: config.get(key)
            for key in (
                "platform",
                "release",
                "architecture",
                "hardware",
                "memory_bytes",
                "cpus",
                "macos",
                "tools",
            )
        }
        session_corpus = {
            (row["suite"], row["file"], row["original_sha256"], row["original_bytes"])
            for row in report["files"]
        }
        if machine is not None and machine != session_machine:
            raise ValueError(
                "Machine or verification toolchain differs between sessions"
            )
        if corpus is not None and corpus != session_corpus:
            raise ValueError("Input files differ between sessions")
        machine, corpus = session_machine, session_corpus
        modes = sorted({row["backend"] for row in report["runs"]})
        duplicates = recorded.intersection(modes)
        if duplicates - replace:
            raise ValueError("Repeated backend requires explicit --replace")
        if duplicates:
            files = [row for row in files if row["backend"] not in duplicates]
            runs = [row for row in runs if row["backend"] not in duplicates]
            for session in sessions:
                session["backends"] = [
                    mode for mode in session["backends"] if mode not in duplicates
                ]
            replaced.update(duplicates)
        recorded.update(modes)
        sessions.append(
            {
                "id": number,
                "backends": modes,
                "recorded_backends": modes,
                "configuration": config,
            }
        )
        files.extend(dict(row, session=number) for row in report["files"])
        runs.extend(dict(row, session=number) for row in report["runs"])
    if replace - replaced:
        raise ValueError("Requested replacement does not occur in two sessions")
    destination.mkdir(parents=True, exist_ok=False)
    provenance = {"sessions": sessions}
    if note:
        provenance["note"] = note
    write_report(destination, files, runs, provenance)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument(
        "--note", help="Provenance note to include in the combined JSON"
    )
    parser.add_argument(
        "--replace",
        action="append",
        default=[],
        metavar="BACKEND",
        help="Keep this repeated backend's later session only",
    )
    parser.add_argument("reports", type=Path, nargs="+")
    args = parser.parse_args()
    merge(args.reports, args.out, set(args.replace), args.note)


if __name__ == "__main__":
    main()
