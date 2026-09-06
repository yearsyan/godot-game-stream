#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-2.1-or-later
"""Run official Godot, decode streamed video and verify GSI1 input end to end."""
import argparse
import json
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    launch = parser.add_mutually_exclusive_group(required=True)
    launch.add_argument("--godot", type=Path)
    launch.add_argument("--executable", type=Path, help="Run an exported copy of the demo")
    parser.add_argument("--project", type=Path, default=ROOT)
    parser.add_argument("--output", type=Path, default=ROOT / "build/smoke")
    parser.add_argument("--ffprobe", default=shutil.which("ffprobe"))
    args = parser.parse_args()
    if not args.ffprobe:
        parser.error("ffprobe is required for independent decoder validation")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    (ROOT / "build/.gdignore").touch()
    command = [str(args.executable.resolve())] if args.executable else [str(args.godot.resolve()), "--path", str(args.project.resolve())]
    if sys.platform == "darwin":
        command += ["--rendering-driver", "metal"]
    command += ["--", "--smoke"]
    log = output / "godot.log"
    with log.open("w") as stream:
        process = subprocess.Popen(command, stdout=stream, stderr=subprocess.STDOUT)
        try:
            deadline = time.monotonic() + 20
            connection = None
            while time.monotonic() < deadline and process.poll() is None:
                try:
                    connection = socket.create_connection(("127.0.0.1", 20831), timeout=0.5)
                    break
                except OSError:
                    time.sleep(0.1)
            if connection is None:
                raise RuntimeError(f"No listener; inspect {log}")
            with connection, (output / "sample.h264").open("wb") as video:
                connection.settimeout(1)
                connection.sendall((ROOT / "protocol/fixtures/mouse-left.bin").read_bytes())
                release = bytearray((ROOT / "protocol/fixtures/mouse-left.bin").read_bytes())
                release[8] = 0
                connection.sendall(release)
                deadline = time.monotonic() + 6
                while time.monotonic() < deadline:
                    try:
                        data = connection.recv(65536)
                    except socket.timeout:
                        continue
                    if not data:
                        break
                    video.write(data)
            process.wait(timeout=15)
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
    if process.returncode != 0:
        raise RuntimeError(f"Godot exited {process.returncode}; inspect {log}")
    text = log.read_text()
    if "game_stream_demo_click=1" not in text:
        raise RuntimeError(f"Remote input did not reach the demo; inspect {log}")
    if "SCRIPT ERROR" in text or "ERROR:" in text:
        raise RuntimeError(f"Godot reported errors; inspect {log}")
    result = subprocess.run([args.ffprobe, "-v", "error", "-count_frames",
        "-select_streams", "v:0", "-show_entries", "stream=codec_name,width,height,nb_read_frames",
        "-of", "json", str(output / "sample.h264")], capture_output=True, text=True, check=True)
    info = json.loads(result.stdout)["streams"][0]
    if result.stderr or int(info.get("nb_read_frames", 0)) < 30 or info["codec_name"] != "h264":
        raise RuntimeError(f"Invalid video: {info}, {result.stderr}")
    info["remote_input"] = "passed"
    (output / "result.json").write_text(json.dumps(info, indent=2) + "\n")
    print(json.dumps(info, indent=2))


if __name__ == "__main__":
    main()
