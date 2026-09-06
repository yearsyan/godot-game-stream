#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-2.1-or-later
# Copyright (C) 2026 yearsyan and contributors

"""Run clang-format on Windows, Linux or macOS.

Usage:
    python scripts/format.py           Format all .c/.h files under src/
    python scripts/format.py --check   Check without modifying files (for CI)
"""
from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SOURCE_DIRS = ("src", "include")  # Reserve include/ for future public headers.


def find_sources() -> list[Path]:
    files: list[Path] = []
    for d in SOURCE_DIRS:
        base = ROOT / d
        if base.is_dir():
            files.extend(p for p in base.rglob("*") if p.suffix in (".c", ".h"))
    return sorted(files)


def main() -> int:
    ap = argparse.ArgumentParser(description="Format C sources with clang-format")
    ap.add_argument("--check", action="store_true", help="Check without modifying files (for CI)")
    args = ap.parse_args()

    if shutil.which("clang-format") is None:
        print(
            "error: clang-format is not installed. Install it with: "
            "apt install clang-format / brew install clang-format / "
            "pip install clang-format",
            file=sys.stderr,
        )
        return 1

    files = find_sources()
    if not files:
        print("no source files found")
        return 0

    if args.check:
        failed = False
        for f in files:
            r = subprocess.run(
                ["clang-format", "--dry-run", "--Werror", str(f)],
                capture_output=True,
            )
            if r.returncode != 0:
                print(f"not formatted: {f.relative_to(ROOT)}")
                failed = True
        if failed:
            print("error: run python scripts/format.py to fix formatting", file=sys.stderr)
            return 1
        print(f"ok: {len(files)} files are formatted")
        return 0

    subprocess.check_call(["clang-format", "-i", *(str(f) for f in files)])
    print(f"formatted {len(files)} files")
    return 0


if __name__ == "__main__":
    sys.exit(main())
