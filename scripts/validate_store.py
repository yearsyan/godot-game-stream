#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-2.1-or-later
"""Download pinned official Godot tools and validate the assembled store package."""
import hashlib
from pathlib import Path
import subprocess
import sys
import zipfile

ROOT = Path(__file__).resolve().parents[1]
BASE = "https://github.com/godotengine/godot/releases/download/4.6.2-stable/"
TOOLS = {
    "win32": ("Godot_v4.6.2-stable_win64.exe.zip", "14293422efb54b24a51f79d4cb55ab4001ef3d936e064a6c8af32e1f984024be"),
    "linux": ("Godot_v4.6.2-stable_linux.x86_64.zip", "30e6b6d141f0cd5bebd629ad1d0ef1324e60091bb20662d026b402ba58c59937"),
    "darwin": ("Godot_v4.6.2-stable_macos.universal.zip", "666b2a64e4b5c59db0e4974605b888eb72eb7d4e60e870d2be6cc19727b50807"),
}


def fetch(name, digest, folder):
    path = folder / name
    if not path.exists():
        partial = folder / (name + ".partial")
        subprocess.run(["curl", "--fail", "--location", "--retry", "3", BASE + name,
                        "--output", str(partial)], check=True)
        partial.rename(path)
    with path.open("rb") as stream:
        if hashlib.file_digest(stream, "sha256").hexdigest() != digest:
            raise ValueError(f"Official tool checksum mismatch: {name}")
    return path


def main():
    folder = ROOT / "build/store-tools"
    folder.mkdir(parents=True, exist_ok=True)
    editor = fetch(*TOOLS[sys.platform], folder)
    templates = fetch("Godot_v4.6.2-stable_export_templates.tpz",
                      "942366dc4e27e7686a99da4d3cfb1b8ae8d3eb9444f6d8217eef16245b599ef2", folder)
    with zipfile.ZipFile(editor) as bundle:
        bundle.extractall(folder / "editor")
        for info in bundle.infolist():
            mode = (info.external_attr >> 16) & 0o777
            if mode:
                (folder / "editor" / info.filename).chmod(mode)
    with zipfile.ZipFile(templates) as bundle:
        target = {"darwin": "macos.zip", "win32": "windows_release_x86_64.exe", "linux": "linux_release.x86_64"}[sys.platform]
        bundle.extract("templates/" + target, folder)
    filename = {"darwin": "Godot.app/Contents/MacOS/Godot", "win32": "Godot_v4.6.2-stable_win64.exe",
                "linux": "Godot_v4.6.2-stable_linux.x86_64"}[sys.platform]
    executable = folder / "editor" / filename
    executable.chmod(0o755)
    subprocess.run([sys.executable, str(ROOT / "scripts/package_store.py")], check=True)
    subprocess.run([sys.executable, str(ROOT / "tests/check_store_export.py"), "--godot", str(executable),
                    "--templates", str(folder / "templates"), "--package",
                    str(ROOT / "dist/store/godot-game-stream-0.1.0-desktop.zip")], check=True)


if __name__ == "__main__":
    main()
