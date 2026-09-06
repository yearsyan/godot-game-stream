#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-2.1-or-later
"""Install the store ZIP in a clean project and verify a relocated native export."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import zipfile

ROOT = Path(__file__).resolve().parents[1]


def run(command, log, env):
    with log.open("w", encoding="utf-8") as output:
        result = subprocess.run(list(map(str, command)), stdout=output,
                                stderr=subprocess.STDOUT, timeout=180, env=env)
    text = log.read_text(encoding="utf-8", errors="replace")
    if result.returncode or "SCRIPT ERROR" in text or "ERROR:" in text:
        raise RuntimeError(f"Command failed ({result.returncode}); inspect {log}\n{text[-5000:]}")
    return text


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--godot", type=Path, required=True)
    parser.add_argument("--templates", type=Path, required=True,
                        help="Extracted official 4.6.2 standard templates directory")
    parser.add_argument("--package", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=ROOT / "build/store-validation")
    args = parser.parse_args()
    work = args.output.resolve()
    work.mkdir(parents=True, exist_ok=True)
    project = work / "project"
    if project.exists():
        raise ValueError("Use a fresh output directory for each verification")
    project.mkdir()
    with zipfile.ZipFile(args.package) as bundle:
        bundle.extractall(project)
    (project / "project.godot").write_text('''config_version=5
[application]
config/name="Game Stream Export Check"
run/main_scene="res://probe.tscn"
config/features=PackedStringArray("4.6", "Forward Plus")
[editor_plugins]
enabled=PackedStringArray("res://addons/game_stream/plugin.cfg")
[rendering]
renderer/rendering_method="forward_plus"
''')
    (project / "probe.gd").write_text('''extends Node
func _ready() -> void:
    if not Engine.has_singleton("GameStream"):
        push_error("Packaged GameStream singleton is missing")
        get_tree().quit(1)
        return
    var stream: Object = Engine.get_singleton("GameStream")
    var stats: Dictionary = stream.call("stats")
    print("GAME_STREAM_EXPORTED_LIBRARY_OK ", stats)
    await get_tree().create_timer(1.0).timeout
    get_tree().quit()
''')
    (project / "probe.tscn").write_text('''[gd_scene load_steps=2 format=3]
[ext_resource type="Script" path="res://probe.gd" id="1"]
[node name="Probe" type="Node"]
script = ExtResource("1")
''')
    target = "macos" if sys.platform == "darwin" else "windows" if sys.platform == "win32" else "linux"
    export_platform = {"macos": "macOS", "windows": "Windows Desktop", "linux": "Linux"}[target]
    filename = {"macos": "GameStreamCheck.zip", "windows": "GameStreamCheck.exe", "linux": "GameStreamCheck.x86_64"}[target]
    template = args.templates.resolve() / {"macos": "macos.zip", "windows": "windows_release_x86_64.exe", "linux": "linux_release.x86_64"}[target]
    if not template.is_file():
        raise ValueError(f"Missing official export template: {template}")
    exported = work / "export"
    exported.mkdir()
    preset = f'''[preset.0]
name="Desktop"
platform="{export_platform}"
runnable=true
export_filter="all_resources"
include_filter=""
exclude_filter=""
export_path=""
script_export_mode=2
[preset.0.options]
custom_template/release={json.dumps(template.as_posix())}
binary_format/architecture="{'arm64' if target == 'macos' else 'x86_64'}"
application/bundle_identifier="org.godotgamestream.exportcheck"
application/min_macos_version_arm64="26.0"
codesign/codesign=1
codesign/identity="-"
'''
    (project / "export_presets.cfg").write_text(preset)
    env = os.environ.copy()
    for key in ("FFMPEG_DIR", "LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH", "DYLD_FALLBACK_LIBRARY_PATH", "GAME_STREAM"):
        env.pop(key, None)
    godot = args.godot.resolve()
    run([godot, "--headless", "--editor", "--path", project, "--max-fps", "30", "--quit-after", "300"], work / "import.log", env)
    run([godot, "--headless", "--path", project, "--export-release", "Desktop", exported / filename], work / "export.log", env)
    relocated = work / "relocated"
    if target == "macos":
        relocated.mkdir()
        subprocess.run(["ditto", "-x", "-k", str(exported / filename), str(relocated)], check=True)
        executable = next(relocated.glob("*.app/Contents/MacOS/*"))
    else:
        shutil.copytree(exported, relocated)
        executable = relocated / filename
        executable.chmod(0o755)
    # Hide the source project to rule out accidental loading from its bin directory.
    project.rename(work / "source-project-after-export")
    text = run([executable, "--headless"], work / "runtime.log", env)
    if "GAME_STREAM_EXPORTED_LIBRARY_OK" not in text:
        raise RuntimeError("The exported application did not confirm the native singleton")
    record = {"platform": target, "godot": "4.6.2", "package_sha256": hashlib.sha256(args.package.read_bytes()).hexdigest(),
              "fresh_editor_import": "passed", "relocated_release_export_native_loading": "passed",
              "hardware_encoding": "not exercised by this dependency check"}
    (work / "result.json").write_text(json.dumps(record, indent=2) + "\n")
    print(json.dumps(record, indent=2))


if __name__ == "__main__":
    main()
