#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-2.1-or-later
"""Import a fresh copy and allow editor documentation to settle before quitting."""
import argparse
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--godot", type=Path, required=True)
    args = parser.parse_args()
    output = ROOT / "build/editor-smoke"
    output.mkdir(parents=True, exist_ok=True)
    (ROOT / "build/.gdignore").touch()
    log = output / "godot.log"
    with tempfile.TemporaryDirectory(prefix="game-stream-editor-") as directory:
        scratch = Path(directory)
        for name in ("addons", "examples"):
            shutil.copytree(ROOT / name, scratch / name,
                            ignore=shutil.ignore_patterns("*.import"))
        shutil.copy2(ROOT / "project.godot", scratch / "project.godot")
        # Immediate --import/--quit can hit Godot issue #111645 during cleanup.
        # Limiting FPS makes this a ten-second run, not 300 unpaced frames.
        command = [str(args.godot.resolve()), "--headless", "--editor", "--path",
                   str(scratch), "--max-fps", "30", "--quit-after", "300"]
        with log.open("w") as stream:
            result = subprocess.run(command, stdout=stream,
                                    stderr=subprocess.STDOUT, timeout=45)
    text = log.read_text()
    if result.returncode or "SCRIPT ERROR" in text or "ERROR:" in text:
        raise RuntimeError(f"Editor check failed ({result.returncode}); inspect {log}")
    print(f"Fresh editor import and exit passed; log: {log}")


if __name__ == "__main__":
    main()
