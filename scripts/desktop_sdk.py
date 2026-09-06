#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-2.1-or-later
"""Build an LGPL FFmpeg 9 SDK with NVENC, AMF and QSV on native desktop hosts."""
import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tarfile

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / "build/desktop-sdk"
PREFIX = ROOT / "build/ffmpeg"
SOURCES = {
    "ffmpeg": ("https://ffmpeg.org/releases/ffmpeg-9.0.1.tar.xz",
               "cf38e0e28c7e5605942c4a77755349b0145804a397af37eb1fb4c77cb237f635"),
    "libvpl": ("https://github.com/intel/libvpl/archive/refs/tags/v2.17.0.tar.gz",
               "4de3e2faf1e8307fb282e4a43f443191810f6a6b0a484fffa7995ba1c814c6ec"),
    "nv-codec-headers": ("https://github.com/FFmpeg/nv-codec-headers/archive/refs/tags/n13.0.19.0.tar.gz",
                         "86d15d1a7c0ac73a0eafdfc57bebfeba7da8264595bf531cf4d8db1c22940116"),
    "amf": ("https://github.com/GPUOpen-LibrariesAndSDKs/AMF/releases/download/v1.5.2/AMF-headers-v1.5.2.tar.gz",
            "d3c12eb324edf05e214608b6a395a51dd95770ed9d45520185d6c3a206811c99"),
}


def run(command, cwd=WORK, **kwargs):
    print("+", " ".join(map(str, command)), flush=True)
    subprocess.run(command, cwd=cwd, check=True, **kwargs)


def main():
    if sys.platform not in ("linux", "win32"):
        raise ValueError("This helper requires a native Linux or Windows x86_64 host")
    WORK.mkdir(parents=True, exist_ok=True)
    (ROOT / "build/.gdignore").touch()
    downloads = WORK / "sources"
    downloads.mkdir(exist_ok=True)
    trees = {}
    for name, (url, digest) in SOURCES.items():
        path = downloads / (name + (".tar.xz" if url.endswith(".xz") else ".tar.gz"))
        if not path.exists():
            run(["curl", "--fail", "--location", "--retry", "3", url, "--output", str(path)])
        if hashlib.sha256(path.read_bytes()).hexdigest() != digest:
            raise ValueError(f"Source checksum mismatch: {name}")
        tree = WORK / name
        if not tree.exists():
            tree.mkdir()
            with tarfile.open(path) as bundle:
                bundle.extractall(tree, filter="data")
        entries = list(tree.iterdir())
        trees[name] = entries[0] if len(entries) == 1 and entries[0].is_dir() else tree

    windows = sys.platform == "win32"
    if windows:
        sys.path.insert(0, str(ROOT / "clients/mirctl/scripts"))
        from build import activate_msvc
        compiler = activate_msvc()
    cmake = ["cmake", "-S", str(trees["libvpl"]), "-B", str(WORK / "vpl-build"),
             "-G", "Ninja", "-DCMAKE_BUILD_TYPE=Release", f"-DCMAKE_INSTALL_PREFIX={PREFIX}",
             "-DCMAKE_INSTALL_LIBDIR=lib", "-DBUILD_SHARED_LIBS=ON", "-DBUILD_TESTS=OFF",
             "-DBUILD_EXAMPLES=OFF", "-DINSTALL_EXAMPLES=OFF", "-DBUILD_EXPERIMENTAL=OFF"]
    if windows:
        cmake += ["-DUSE_MSVC_STATIC_RUNTIME=ON", "-DCMAKE_INSTALL_SYSTEM_RUNTIME_LIBS_SKIP=ON"]
    run(cmake)
    run(["cmake", "--build", str(WORK / "vpl-build"), "--parallel", str(os.cpu_count() or 2)])
    run(["cmake", "--install", str(WORK / "vpl-build")])
    include = PREFIX / "include"
    shutil.copytree(trees["nv-codec-headers"] / "include/ffnvcodec", include / "ffnvcodec", dirs_exist_ok=True)
    amf_version = next(trees["amf"].rglob("core/Version.h"))
    shutil.copytree(amf_version.parent.parent, include / "AMF", dirs_exist_ok=True)
    pkgconfig = PREFIX / "lib/pkgconfig"
    (pkgconfig / "ffnvcodec.pc").write_text(
        f"prefix={PREFIX.as_posix()}\nincludedir=${{prefix}}/include\n"
        "Name: ffnvcodec\nDescription: FFmpeg NVIDIA codec headers\nVersion: 13.0.19.0\n"
        "Cflags: -I${includedir}\n")
    command = ["sh", trees["ffmpeg"].joinpath("configure").as_posix(), f"--prefix={PREFIX.as_posix()}",
               "--disable-autodetect", "--disable-everything", "--disable-gpl", "--disable-nonfree",
               "--disable-version3", "--disable-static", "--enable-shared", "--disable-doc",
               "--disable-programs", "--disable-network", "--disable-avdevice", "--disable-avfilter",
               "--enable-avcodec", "--enable-avformat", "--enable-avutil", "--enable-swscale",
               "--enable-swresample", "--enable-decoder=h264,hevc", "--enable-parser=h264,hevc",
               "--enable-ffnvcodec", "--enable-nvenc", "--enable-amf", "--enable-libvpl",
               "--enable-encoder=h264_nvenc,hevc_nvenc,av1_nvenc,h264_amf,hevc_amf,av1_amf,h264_qsv,hevc_qsv,av1_qsv",
               f"--extra-cflags=-I{include.as_posix()}"]
    if windows:
        command += ["--toolchain=msvc", "--arch=x86_64", "--target-os=win32",
                    "--enable-d3d11va", "--enable-dxva2"]
    else:
        command += ["--enable-vaapi", "--enable-libdrm"]
    compile_dir = WORK / "ffmpeg-build"
    compile_dir.mkdir(exist_ok=True)
    env = os.environ.copy()
    if windows:
        bash = Path(os.environ.get("MSYS2_LOCATION", "C:/msys64")) / "usr/bin/bash.exe"
        # MSVC link.exe must precede the unrelated MSYS link utility.
        setup = (f'export PATH="$(cygpath -u {shlex.quote(compiler.as_posix())}):/usr/bin:$PATH"\n'
                 f'export PKG_CONFIG_PATH="$(cygpath -u {shlex.quote(pkgconfig.as_posix())})"\n')
        shell = setup + "set -e\n" + shlex.join(command) + f"\nmake -j{os.cpu_count() or 2}\nmake install\n"
        run([str(bash), "--noprofile", "--norc", "-c", shell], cwd=compile_dir, env=env)
    else:
        env["PKG_CONFIG_PATH"] = str(pkgconfig)
        run(command, cwd=compile_dir, env=env)
        run(["make", f"-j{os.cpu_count() or 2}"], cwd=compile_dir)
        run(["make", "install"], cwd=compile_dir)
    licenses = PREFIX / "licenses"
    licenses.mkdir(exist_ok=True)
    for name, tree in trees.items():
        for pattern in ("LICENSE*", "COPYING*", "third-party-programs.txt"):
            for path in tree.glob(pattern):
                if path.is_file():
                    shutil.copy2(path, licenses / f"{name}-{path.name}")
    shutil.copy2(trees["ffmpeg"] / "COPYING.LGPLv2.1", licenses / "COPYING.LGPLv2.1")
    # The AMF header archive carries its MIT notice in the header preamble.
    shutil.copy2(amf_version, licenses / "AMF-Version.h")
    metadata = {"sources": SOURCES, "configure": command, "source_tree": str(trees["ffmpeg"]),
                "runtime": "GPU drivers are supplied by the operating system; Linux also requires libva and libdrm."}
    (PREFIX / "build-info.json").write_text(json.dumps(metadata, indent=2) + "\n")
    (downloads / "build-info.json").write_text(json.dumps(metadata, indent=2) + "\n")
    for name in ("config.h", "config_components.h", "ffbuild/config.mak", "ffbuild/config.log"):
        path = compile_dir / name
        shutil.copy2(path, downloads / path.name)
    print(f"SDK: {PREFIX}")


if __name__ == "__main__":
    main()
