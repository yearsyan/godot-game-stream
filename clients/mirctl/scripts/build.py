#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-2.1-or-later
# Copyright (C) 2026 yearsyan and contributors

"""Build mirctl on Windows, Linux or macOS.

Usage:
    python scripts/build.py                Release build (Conan install + CMake)
    python scripts/build.py --build-type debug
    python scripts/build.py --offline      Use cached Conan dependencies only
    python scripts/build.py --skip-conan   Skip Conan install if already configured
"""
from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
from glob import glob
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def run(cmd: list[str]) -> None:
    print("+", " ".join(cmd))
    subprocess.check_call(cmd, cwd=ROOT)


def find_vcvarsall() -> Path:
    vs_install_dir = os.environ.get("VSINSTALLDIR")
    if vs_install_dir:
        candidate = Path(vs_install_dir) / "VC" / "Auxiliary" / "Build" / "vcvarsall.bat"
        if candidate.is_file():
            return candidate

    program_files_x86 = os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)")
    vswhere = (
        Path(program_files_x86)
        / "Microsoft Visual Studio"
        / "Installer"
        / "vswhere.exe"
    )
    if vswhere.is_file():
        result = subprocess.run(
            [
                str(vswhere),
                "-latest",
                "-products",
                "*",
                "-requires",
                "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
                "-property",
                "installationPath",
            ],
            check=False,
            capture_output=True,
            text=True,
        )
        for line in result.stdout.splitlines():
            candidate = (
                Path(line.strip()) / "VC" / "Auxiliary" / "Build" / "vcvarsall.bat"
            )
            if candidate.is_file():
                return candidate

    patterns = [
        r"C:\Program Files\Microsoft Visual Studio\*\*\VC\Auxiliary\Build\vcvarsall.bat",
        r"C:\Program Files (x86)\Microsoft Visual Studio\*\*\VC\Auxiliary\Build\vcvarsall.bat",
    ]
    candidates = [Path(path) for pattern in patterns for path in glob(pattern)]
    if candidates:
        return max(candidates, key=lambda path: path.stat().st_mtime_ns)
    raise RuntimeError("MSVC x64 toolchain not found; install Visual Studio C++ Build Tools")


def activate_msvc() -> Path:
    """Initialize cl, link and the Windows SDK in a regular terminal."""
    cl = shutil.which("cl.exe")
    if (
        cl
        and shutil.which("rc.exe")
        and shutil.which("mt.exe")
        and os.environ.get("INCLUDE")
        and os.environ.get("LIB")
    ):
        return Path(cl).resolve().parent

    vcvarsall = find_vcvarsall()
    command = f'call "{vcvarsall}" amd64 >nul && set'
    result = subprocess.run(
        command,
        shell=True,
        check=False,
        capture_output=True,
        text=True,
        errors="replace",
    )
    if result.returncode:
        raise RuntimeError(f"Failed to initialize the MSVC environment:\n{result.stderr.strip()}")

    # PowerShell environments can contain both PATH and Path, and cmd.exe prints
    # both. Prefer uppercase names because vcvarsall updates the uppercase PATH.
    values: dict[str, tuple[str, str]] = {}
    for line in result.stdout.splitlines():
        name, separator, value = line.partition("=")
        if not separator or not name:
            continue
        key = name.upper()
        if key not in values or name == key:
            values[key] = (name, value)
    for key, (_, value) in values.items():
        os.environ[key] = value

    cl = shutil.which("cl.exe")
    if not cl or not shutil.which("rc.exe") or not shutil.which("mt.exe"):
        raise RuntimeError("MSVC initialized, but cl.exe, rc.exe or mt.exe is missing")
    return Path(cl).resolve().parent


def write_ffmpeg_msvc_profile(msvc_bin: Path) -> Path:
    """Preserve the cl/link directory for the FFmpeg recipe's MSYS2 build."""
    profile = ROOT / "build" / "conan-msvc-env.profile"
    profile.parent.mkdir(parents=True, exist_ok=True)
    profile.write_text(
        "[buildenv]\n"
        f"ffmpeg/*:PATH=+(path){msvc_bin.as_posix()}\n",
        encoding="utf-8",
    )
    return profile


def main() -> int:
    ap = argparse.ArgumentParser(description="Build mirctl and its dependencies")
    ap.add_argument(
        "--build-type", choices=("release", "debug"), default="release"
    )
    ap.add_argument("--offline", action="store_true", help="Use only the local Conan cache")
    ap.add_argument("--skip-conan", action="store_true", help="Skip Conan install")
    ap.add_argument("--lockfile", type=Path, help="Resolve dependencies from a Conan lockfile")
    args = ap.parse_args()

    build_type = "Release" if args.build_type == "release" else "Debug"
    msvc_profile = None
    if sys.platform == "win32":
        msvc_profile = write_ffmpeg_msvc_profile(activate_msvc())

    if not args.skip_conan:
        (ROOT / "build").mkdir(exist_ok=True)
        conan_command = [
            "conan", "install", ".",
            "--build=missing",
            "-s:h", f"build_type={build_type}",
            "-c:h", "tools.cmake.cmaketoolchain:generator=Ninja",
            "--lockfile-out", str(ROOT / "build/conan.lock"),
            "--format=json", "--out-file", str(ROOT / "build/conan-graph.json"),
        ]
        if args.lockfile:
            conan_command.extend(["--lockfile", str(args.lockfile.resolve())])
        if args.offline:
            conan_command.append("--no-remote")
        if sys.platform == "win32":
            conan_command.extend(
                [
                    "-pr:h", "default",
                    "-pr:h", str(msvc_profile),
                    "-s:h", "compiler.runtime=static",
                    "-o",
                    "ffmpeg/*:enable_hardware_accelerators="
                    "h264_d3d11va,h264_d3d11va2,h264_dxva2,"
                    "hevc_d3d11va,hevc_d3d11va2,hevc_dxva2,"
                    "av1_d3d11va,av1_d3d11va2,av1_dxva2",
                ]
            )
        elif sys.platform == "darwin":
            conan_command.extend(
                [
                    "-o",
                    "ffmpeg/*:enable_hardware_accelerators="
                    "h264_videotoolbox,hevc_videotoolbox,av1_videotoolbox",
                ]
            )
        elif sys.platform == "linux":
            # These options exist only on Linux; the recipes remove them on
            # Windows/macOS, so they cannot live in conanfile.txt. FFmpeg's
            # default PulseAudio 14.2 dependency conflicts with SDL's 17.0 pin.
            # ALSA/XCB/Xlib require avdevice. This video-only client also has no
            # use for VAAPI/VDPAU or SDL audio backends in its current build.
            conan_command.extend(
                [
                    "-o", "ffmpeg/*:with_libalsa=False",
                    "-o", "ffmpeg/*:with_pulse=False",
                    "-o", "ffmpeg/*:with_xcb=False",
                    "-o", "ffmpeg/*:with_xlib=False",
                    "-o", "ffmpeg/*:with_vaapi=False",
                    "-o", "ffmpeg/*:with_vdpau=False",
                    "-o", "sdl/*:alsa=False",
                    "-o", "sdl/*:pulseaudio=False",
                    "-o", "sdl/*:sndio=False",
                    "-o", "sdl/*:wayland=False",
                    "-o", "sdl/*:dbus=False",
                    "-o", "sdl/*:libudev=False",
                    "-o", "sdl/*:opengles=False",
                ]
            )
        run(conan_command)

    dependency_dir = ROOT / "build" / build_type / "generators"
    toolchain = dependency_dir / "conan_toolchain.cmake"
    if not toolchain.is_file():
        raise RuntimeError(f"Conan toolchain not found: {toolchain}")

    app_build_dir = ROOT / "build" / "app" / build_type
    configure_command = [
        "cmake",
        "-S", str(ROOT),
        "-B", str(app_build_dir),
        f"-DCMAKE_TOOLCHAIN_FILE={toolchain}",
        f"-DCMAKE_BUILD_TYPE={build_type}",
        "-DCMAKE_POLICY_DEFAULT_CMP0091=NEW",
    ]
    if sys.platform == "win32":
        # NMake ships with MSVC, avoiding a separate system Ninja dependency.
        configure_command.extend(["-G", "NMake Makefiles"])
    run(configure_command)
    run(["cmake", "--build", str(app_build_dir), "--config", build_type])

    # Search recursively because executable paths vary between CMake generators.
    exe_name = "mirctl.exe" if sys.platform == "win32" else "mirctl"
    found = [
        Path(path)
        for path in glob(str(ROOT / "build" / "**" / exe_name), recursive=True)
    ]
    if found:
        built_exe = max(found, key=lambda path: path.stat().st_mtime_ns)
        dist_dir = ROOT / "dist"
        dist_dir.mkdir(exist_ok=True)
        published_exe = dist_dir / exe_name
        shutil.copy2(built_exe, published_exe)
        print(f"\nBuild completed: {built_exe}")
        print(f"Standalone executable: {published_exe}")
    else:
        print("\nBuild completed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
