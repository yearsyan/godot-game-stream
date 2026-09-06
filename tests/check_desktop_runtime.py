#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-2.1-or-later
"""Check packaged library loading and all advertised hardware encoder wrappers."""
import ctypes
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
system = "windows" if sys.platform == "win32" else "linux"
folder = ROOT / "addons/game_stream/bin" / system / "x86_64"
record = json.loads((folder / "platform.json").read_text())
dll_directory = os.add_dll_directory(str(folder)) if system == "windows" else None
extension = ctypes.CDLL(str(folder / record["library"]))
codec_path = next(folder.glob("avcodec-*.dll" if system == "windows" else "libavcodec.so.*"))
codec = ctypes.CDLL(str(codec_path))
codec.avcodec_license.restype = ctypes.c_char_p
assert codec.avcodec_license().startswith(b"LGPL"), "FFmpeg must be LGPL"
codec.avcodec_find_encoder_by_name.argtypes = [ctypes.c_char_p]
codec.avcodec_find_encoder_by_name.restype = ctypes.c_void_p
for backend in ("nvenc", "amf", "qsv"):
    for wire in ("h264", "hevc", "av1"):
        name = f"{wire}_{backend}"
        assert codec.avcodec_find_encoder_by_name(name.encode()), f"Missing encoder wrapper: {name}"
        print(f"Compiled encoder: {name}")
if system == "linux":
    env = os.environ.copy()
    env["LD_LIBRARY_PATH"] = str(folder)
    for path in folder.glob("*.so*"):
        result = subprocess.check_output(["ldd", str(path)], text=True, env=env)
        if "not found" in result:
            raise RuntimeError(result)
    print(subprocess.check_output(["ldd", str(folder / record["library"])], text=True))
print("Packaged dependencies load successfully; hardware availability requires a native GPU test.")
