# Windows and Linux release artifacts

The v0.1.0 release includes native x86_64 builds of the Godot addon and mirctl
for Windows and Linux. Choose the platform suffix `windows-x86_64` or
`linux-x86_64` when downloading either binary ZIP.

## Requirements

- Godot 4.6.2 standard precision, using Forward+ and a hardware GPU.
- Windows 10 or later, x86_64; the addon may require the Microsoft Visual C++
  2015-2022 x64 Redistributable. The client uses the static MSVC runtime.
- Linux x86_64, glibc 2.35 or later (built on Ubuntu 22.04), with the distribution's
  libva, libva-drm and libdrm runtime libraries. mirctl uses X11 or Xwayland and
  system OpenGL/X11 libraries. Native Wayland and audio backends are disabled.
- A working NVIDIA NVENC, AMD AMF or Intel QSV hardware encoder and vendor driver.
  NVIDIA headers target Video Codec SDK 13.0 (R570 or newer drivers).
  Intel QSV needs a compatible Intel GPU media runtime; the VPL dispatcher is bundled.
  AMD requires the AMF runtime provided by the driver. Drivers are not bundled.

The addon includes LGPL FFmpeg 9.0.1 shared libraries and the MIT-licensed
Intel VPL dispatcher. NVENC and AMF interfaces load the system GPU driver.
H.264, HEVC and AV1 wrappers are compiled for all three backends; actual codec
support depends on the GPU. For example, RTX 3080 supports HEVC encoding but
does not support AV1 encoding. There is no x264 software fallback.

## Provenance and validation

The maintainer reports Windows and Linux streaming validation in GachaGameProducer.
The supplemental standalone packages are built on GitHub's native Windows 2022
and Ubuntu 22.04 runners. Release checks include Rust tests/Clippy, C protocol
fixtures, FFmpeg license/encoder inventory, dependency loading and a paced Godot
editor smoke check. The separate Store package validation workflow also exports
a release application and checks native loading after moving it to another
directory. Hosted runners do not validate hardware video encoding; that requires
the target GPU and driver.

The original v0.1.0 tag and macOS assets remain unchanged. Supplemental build
records identify the newer source commit used for the desktop packaging changes.
Use `godot-game-stream-0.1.0-<platform>-source.zip` for these binaries, together with:

- `ffmpeg-<platform>-source.zip`: complete source used for the shared FFmpeg build.
- `native-<platform>-dependencies.zip`: checksum-pinned FFmpeg, VPL, NVIDIA and AMD
  header sources plus configure logs and build records.
- `rust-<platform>-dependencies.zip`: locked Rust sources.
- `mirctl-0.1.0-<platform>-dependencies.zip`: client sources/recipes, licenses,
  static libraries, application objects and link records for rebuilding/relinking.
- `release-build-<platform>.json` and `SHA256SUMS`: provenance and integrity records.

## Reproduce

Use Python 3.12, Rust, Conan 2.28.1, CMake/Ninja, nasm and pkg-config. Windows
also needs Visual Studio 2022 C++ tools, LLVM/libclang and MSYS2 make/pkgconf.
Linux needs GCC, Clang/libclang and development packages for libva, libdrm,
OpenGL and X11. The workflow lists the exact runner setup.

```sh
python scripts/desktop_release.py
```

This downloads checksum-pinned pug/Godot tools, builds the shared SDK using
`scripts/desktop_sdk.py`, builds the addon through pug, builds mirctl through
Conan/CMake, runs checks and creates the release archives. The workflow is
manually dispatched and produces reviewable artifacts; it does not publish
to GitHub Releases automatically.
