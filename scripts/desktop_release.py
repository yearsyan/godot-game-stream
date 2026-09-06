#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-2.1-or-later
"""Build, validate and package supplemental Windows/Linux release artifacts."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import zipfile

ROOT = Path(__file__).resolve().parents[1]


def run(command, **kwargs):
    print("+", " ".join(map(str, command)), flush=True)
    subprocess.run(command, cwd=ROOT, check=True, **kwargs)


def main():
    windows = sys.platform == "win32"
    target = "windows-x86_64" if windows else "linux-x86_64"
    work = ROOT / "build/release-tools"
    work.mkdir(parents=True, exist_ok=True)
    assets = [
        ("https://github.com/yearsyan/pug/releases/download/v0.2.3/" +
         ("pug-windows.zip" if windows else "pug-linux.tar.gz"),
         "ff182494aa5113d46c86fabe496ce166c9eeb74e609d967b05ce9221d54f2cdf" if windows else
         "d28026d4abecfa4ac52fade181d647ca4f2b83ec444c71509353378db481bace"),
        ("https://github.com/godotengine/godot/releases/download/4.6.2-stable/" +
         ("Godot_v4.6.2-stable_win64.exe.zip" if windows else "Godot_v4.6.2-stable_linux.x86_64.zip"),
         "14293422efb54b24a51f79d4cb55ab4001ef3d936e064a6c8af32e1f984024be" if windows else
         "30e6b6d141f0cd5bebd629ad1d0ef1324e60091bb20662d026b402ba58c59937"),
    ]
    for url, digest in assets:
        path = work / url.rsplit("/", 1)[1]
        if not path.exists():
            run(["curl", "--fail", "--location", "--retry", "3", url, "--output", str(path)])
        if hashlib.sha256(path.read_bytes()).hexdigest() != digest:
            raise ValueError(f"Tool checksum mismatch: {path.name}")
        if path.suffix == ".zip":
            with zipfile.ZipFile(path) as bundle:
                bundle.extractall(work)
        else:
            with tarfile.open(path) as bundle:
                bundle.extractall(work, filter="data")
    pug = next(work.rglob("pug.exe" if windows else "pug"))
    godot = next(work.rglob("Godot_v4.6.2-stable_win64.exe" if windows else "Godot_v4.6.2-stable_linux.x86_64"))
    if not windows:
        pug.chmod(0o755)
        godot.chmod(0o755)
    os.environ["PATH"] = str(pug.parent) + os.pathsep + os.environ["PATH"]
    probe = subprocess.run([str(pug), "--version"], capture_output=True, text=True)
    if probe.returncode:
        if windows:
            raise RuntimeError(probe.stderr)
        # Upstream Linux tools may require a newer glibc than the release baseline.
        run(["cargo", "install", "--git", "https://github.com/yearsyan/pug.git", "--tag", "v0.2.3",
             "--locked", "--root", str(work / "pug-local")])
        os.environ["PATH"] = str(work / "pug-local/bin") + os.pathsep + os.environ["PATH"]
    if windows:
        sys.path.insert(0, str(ROOT / "clients/mirctl/scripts"))
        from build import activate_msvc
        activate_msvc()
    sdk = ROOT / "build/ffmpeg"
    if os.environ.get("GAME_STREAM_REUSE_SDK") != "true" or not (sdk / "build-info.json").is_file():
        run([sys.executable, "scripts/desktop_sdk.py"])
    env = os.environ.copy()
    env["FFMPEG_DIR"] = str(sdk)
    env["PKG_CONFIG_PATH"] = str(sdk / "lib/pkgconfig")
    if windows:
        env["PATH"] = str(sdk / "bin") + os.pathsep + env["PATH"]
        env["LIBCLANG_PATH"] = "C:/Program Files/LLVM/bin"
    else:
        env["LD_LIBRARY_PATH"] = str(sdk / "lib")
    run([sys.executable, "scripts/build.py", "--ffmpeg-dir", str(sdk), "--godot", str(godot)], env=env)
    crate = ["--manifest-path", "native/game_stream/Cargo.toml"]
    run(["cargo", "fmt", *crate, "--check"])
    run(["cargo", "test", "--locked", "--release", *crate], env=env)
    run(["cargo", "clippy", "--locked", "--release", "--all-targets", *crate, "--", "-D", "warnings"], env=env)
    # Run in a fresh process, loading only packaged libraries and system dependencies.
    run([sys.executable, "tests/check_desktop_runtime.py"], env=os.environ.copy())
    run([sys.executable, "tests/smoke_editor.py", "--godot", str(godot)])
    run(["conan", "profile", "detect", "--force"])
    try:
        run([sys.executable, "clients/mirctl/scripts/build.py"])
    except subprocess.CalledProcessError:
        logs = list((Path.home() / ".conan2/p/b").glob("ffmpe*/b/**/config.log"))
        if logs:
            latest = max(logs, key=lambda p: p.stat().st_mtime)
            print(f"FFmpeg dependency configuration log: {latest}", flush=True)
            print(latest.read_text(errors="replace")[-10000:], flush=True)
        raise
    run(["ctest", "--test-dir", "clients/mirctl/build/app/Release", "--output-on-failure"])
    run(["cargo", "vendor", "--locked", *crate, "build/rust-vendor"])
    source = json.loads((sdk / "build-info.json").read_text())["source_tree"]
    run([sys.executable, "scripts/package.py", "--ffmpeg-source", source, "--client", "--supplemental"])
    print(f"Release artifacts ready for review: {target}")


if __name__ == "__main__":
    main()
