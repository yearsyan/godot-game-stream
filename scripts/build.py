#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-2.1-or-later
"""Build with pug, install the addon and generate Godot's dependency manifest."""
import argparse
import ctypes
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
ADDON = ROOT / "addons/game_stream"
COMPONENTS = ("avutil", "swresample", "swscale", "avcodec", "avformat")


def host_target():
    system = {"darwin": "macos", "win32": "windows", "linux": "linux"}.get(sys.platform)
    arch = {"arm64": "arm64", "aarch64": "arm64", "x86_64": "x86_64", "AMD64": "x86_64"}.get(platform.machine())
    if not system or not arch:
        raise ValueError(f"Unsupported build host: {sys.platform}/{platform.machine()}")
    return system, arch


def sdk_libraries(sdk, system):
    folder = sdk / ("bin" if system == "windows" else "lib")
    patterns = ("*.dll",) if system == "windows" else ("*.dylib",) if system == "macos" else ("*.so", "*.so.*")
    files = {p.name: p for pattern in patterns for p in folder.glob(pattern) if p.is_file()}
    for component in COMPONENTS:
        prefix = component + "-" if system == "windows" else "lib" + component + "."
        if not any(name.startswith(prefix) for name in files):
            raise ValueError(f"Missing {component} runtime library in {folder}")
    return files


def inspect_sdk(sdk, system):
    libraries = sdk_libraries(sdk, system)
    dll_dir = os.add_dll_directory(str(sdk / "bin")) if system == "windows" else None
    report = {}
    loaded = []
    try:
        for component in COMPONENTS:
            prefix = component + "-" if system == "windows" else "lib" + component + "."
            path = next(p for name, p in sorted(libraries.items()) if name.startswith(prefix))
            lib = ctypes.CDLL(str(path), mode=ctypes.RTLD_GLOBAL)
            loaded.append(lib)
            license_fn = getattr(lib, component + "_license")
            license_fn.restype = ctypes.c_char_p
            license_text = license_fn().decode()
            if not license_text.startswith("LGPL"):
                raise ValueError(f"Refusing {path.name}: {license_text}; provide an LGPL FFmpeg SDK")
            config_fn = getattr(lib, component + "_configuration")
            config_fn.restype = ctypes.c_char_p
            version_fn = getattr(lib, component + "_version")
            version_fn.restype = ctypes.c_uint
            if component == "avcodec" and version_fn() >> 16 != 63:
                raise ValueError("game_stream requires FFmpeg 9 (libavcodec major 63)")
            report[component] = {"license": license_text, "configuration": config_fn().decode()}
    finally:
        if dll_dir:
            dll_dir.close()
    return libraries, report


def relocate_macos(folder):
    # Bundle must not retain developer-machine Homebrew or SDK paths.
    for library in folder.glob("*.dylib"):
        output = subprocess.check_output(["otool", "-L", str(library)], text=True)
        edits = ["install_name_tool", "-id", "@rpath/" + library.name]
        for line in output.splitlines()[1:]:
            dependency = line.strip().split(" (", 1)[0]
            if dependency.startswith(("/System/Library/", "/usr/lib/")):
                continue
            name = Path(dependency).name
            if not (folder / name).is_file():
                raise ValueError(f"Unbundled dependency: {library.name} -> {dependency}")
            edits += ["-change", dependency, "@loader_path/" + name]
        subprocess.run(edits + [str(library)], check=True)
        subprocess.run(["codesign", "--force", "--sign", "-", str(library)], check=True)


def write_manifest():
    configs = sorted((ADDON / "bin").glob("*/*/platform.json"))
    lines = ['[configuration]', 'entry_symbol = "gdext_rust_init"',
             'compatibility_minimum = "4.6.2"', 'reloadable = false', '', '[libraries]']
    dependencies = []
    for config in configs:
        item = json.loads(config.read_text())
        system, arch = item["platform"], item["arch"]
        feature = f"{system}.{arch}"
        prefix = f"bin/{system}/{arch}/"
        lines.append(f'{feature} = "./{prefix}{item["library"]}"')
        destination = "Contents/Frameworks" if system == "macos" else ""
        entries = ",\n".join(f'    "./{prefix}{name}": "{destination}"' for name in item["dependencies"])
        dependencies.append(f"{feature} = {{\n{entries}\n}}")
    (ADDON / "game_stream.gdextension").write_text("\n".join(lines + ["", "[dependencies]"] + dependencies) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ffmpeg-dir", type=Path, default=os.environ.get("FFMPEG_DIR"))
    parser.add_argument("--godot", type=Path, help="Official Godot 4.6+ editor; forwarded to pug")
    args = parser.parse_args()
    if not args.ffmpeg_dir:
        parser.error("Pass --ffmpeg-dir or set FFMPEG_DIR to an LGPL FFmpeg 9 SDK")
    sdk = args.ffmpeg_dir.resolve()
    system, arch = host_target()
    libraries, report = inspect_sdk(sdk, system)
    env = os.environ.copy()
    env["FFMPEG_DIR"] = str(sdk)
    env["PKG_CONFIG_PATH"] = str(sdk / "lib/pkgconfig") + os.pathsep + env.get("PKG_CONFIG_PATH", "")
    command = ["pug", "extension", "build", "--platform", f"{system}:{arch}"]
    if args.godot:
        command += ["--with-engine", str(args.godot.resolve())]
    crate = ROOT / "native/game_stream"
    subprocess.run(command, cwd=crate, env=env, check=True)
    name = "game_stream.dll" if system == "windows" else "libgame_stream.dylib" if system == "macos" else "libgame_stream.so"
    candidates = [p for p in (crate / "target").rglob(name) if p.parent.name == "release"]
    if not candidates:
        raise ValueError(f"pug did not produce {name}")
    compiled = max(candidates, key=lambda p: p.stat().st_mtime_ns)
    folder = ADDON / "bin" / system / arch
    if folder.exists():
        shutil.rmtree(folder)
    folder.mkdir(parents=True)
    shutil.copy2(compiled, folder / name)
    for libname, path in libraries.items():
        shutil.copy2(path, folder / libname, follow_symlinks=True)
    if system == "macos":
        relocate_macos(folder)
    notice_folder = ADDON / "THIRD_PARTY" / f"ffmpeg-{system}-{arch}"
    notice_folder.mkdir(parents=True, exist_ok=True)
    for path in (sdk / "licenses").glob("*"):
        if path.is_file():
            shutil.copy2(path, notice_folder / path.name)
    (notice_folder / "build.json").write_text(json.dumps(report, indent=2) + "\n")
    (folder / "platform.json").write_text(json.dumps({
        "platform": system, "arch": arch, "library": name, "dependencies": sorted(libraries)
    }, indent=2) + "\n")
    write_manifest()
    print(f"Installed {system}:{arch} addon in {ADDON}")


if __name__ == "__main__":
    main()
