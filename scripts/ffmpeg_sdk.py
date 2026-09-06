#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-2.1-or-later
"""Build a minimal LGPL FFmpeg SDK from an unpacked FFmpeg 9 source release."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--prefix", type=Path, default=ROOT / "build/ffmpeg")
    parser.add_argument("--extra-configure", action="append", default=[])
    args = parser.parse_args()
    source, prefix = args.source.resolve(), args.prefix.resolve()
    if sys.platform == "win32":
        parser.error("Use an LGPL FFmpeg 9 shared SDK on Windows; this helper requires POSIX make.")
    if not (source / "configure").is_file():
        parser.error("--source must contain the FFmpeg configure script")
    for option in args.extra_configure:
        if option.startswith(("--enable-gpl", "--enable-nonfree", "--enable-version3")):
            parser.error("This SDK is built as LGPL-2.1-or-later")
    work = ROOT / "build/ffmpeg-compile"
    work.mkdir(parents=True, exist_ok=True)
    (ROOT / "build/.gdignore").touch()
    command = [str(source / "configure"), f"--prefix={prefix}",
               "--disable-autodetect", "--disable-everything", "--disable-gpl",
               "--disable-nonfree", "--disable-version3", "--disable-static",
               "--enable-shared", "--disable-doc", "--disable-programs",
               "--disable-network", "--disable-avdevice", "--disable-avfilter",
               "--enable-avcodec", "--enable-avformat", "--enable-avutil",
               "--enable-swscale", "--enable-swresample",
               "--enable-decoder=h264,hevc", "--enable-parser=h264,hevc"]
    if sys.platform == "darwin":
        command += ["--enable-videotoolbox", "--enable-encoder=h264_videotoolbox,hevc_videotoolbox",
                    "--install-name-dir=@rpath"]
    command += args.extra_configure
    for cmd in (command, ["make", f"-j{os.cpu_count() or 2}"], ["make", "install"]):
        print("+", " ".join(cmd), flush=True)
        subprocess.run(cmd, cwd=work, check=True)
    licenses = prefix / "licenses"
    licenses.mkdir(exist_ok=True)
    for name in ("LICENSE.md", "COPYING.LGPLv2.1", "COPYING.GPLv2"):
        shutil.copy2(source / name, licenses / name)
    (prefix / "build-info.json").write_text(json.dumps({
        "source": str(source), "configure": command,
        "source_note": "Archive this exact source directory alongside binary releases."
    }, indent=2) + "\n")
    print(f"SDK: {prefix}")


if __name__ == "__main__":
    main()
