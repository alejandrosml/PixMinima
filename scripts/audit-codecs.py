#!/usr/bin/env python3
"""Check active codec dependencies with unsafe code forbidden, including implicit dependencies.

Cargo caps diagnostics for published dependencies, which would otherwise mask lint
failures. A dedicated rustc wrapper below removes that cap for this audit only.
"""
import os
from pathlib import Path
import subprocess
import sys
import tempfile


def compiler(arguments):
    rustc, *args = arguments
    effective = []
    i = 0
    while i < len(args):
        if args[i] == "--cap-lints":
            i += 2
        else:
            if not args[i].startswith("--cap-lints="):
                effective.append(args[i])
            i += 1
    # Preserve other crate-level warnings at their normal severity.
    os.execvp(rustc, [rustc, *effective, "-Funsafe-code"])


def audit():
    project = Path(__file__).resolve().parents[1]
    if os.name == "nt":
        raise SystemExit("Run the codec audit on macOS or Linux.")
    with tempfile.TemporaryDirectory(prefix="pixminima-audit-") as tmp:
        env = dict(os.environ, RUSTC_WRAPPER=str(Path(__file__).resolve()), CARGO_TARGET_DIR=tmp)
        command = ["cargo", "check", "--locked", "--lib"]
        for name in ["pixminima-png", "pixminima-formats", "pixminima-extra", "pixminima-containers"]:
            command += ["-p", name]
        return subprocess.call(command, cwd=project, env=env)


if __name__ == "__main__":
    sys.exit(compiler(sys.argv[1:]) if len(sys.argv) > 1 else audit())
