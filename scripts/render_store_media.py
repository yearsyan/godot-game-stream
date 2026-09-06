#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-2.1-or-later
"""Rasterize editable store artwork and extract an actual decoded stream frame."""
import argparse
from pathlib import Path
import subprocess

import cairosvg

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--stream", type=Path, help="H.264 capture produced by the streaming smoke check")
    args = parser.parse_args()
    media = ROOT / "store/media"
    media.mkdir(parents=True, exist_ok=True)
    cairosvg.svg2png(url=str(ROOT / "store/thumbnail.svg"), write_to=str(media / "thumbnail.png"))
    cairosvg.svg2png(url=str(ROOT / "addons/game_stream/icon.svg"),
                    output_width=512, output_height=512, write_to=str(media / "icon.png"))
    if args.stream:
        subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-i", str(args.stream),
                        "-vf", r"select=eq(n\,90)", "-frames:v", "1", str(media / "stream-demo.png")], check=True)
        if not (media / "stream-demo.png").is_file():
            raise ValueError("The stream did not contain frame 90")
    print(media)


if __name__ == "__main__":
    main()
