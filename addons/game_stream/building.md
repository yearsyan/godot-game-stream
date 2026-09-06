# Building and packaging

## Native addon

Prerequisites: Python 3.11+, Rust with the host target, `pug`, a Godot 4.6+
editor, libclang/bindgen tooling and an **LGPL FFmpeg 9 shared SDK**.
FFmpeg headers, import libraries and runtime libraries must be from one build.
Use native hosts for each OS; this repository does not configure cross compilation.

On macOS a minimal SDK can be built from an unpacked FFmpeg 9.0.1 release:

```sh
python3 scripts/ffmpeg_sdk.py --source /path/to/ffmpeg-9.0.1
python3 scripts/build.py --ffmpeg-dir build/ffmpeg --godot /path/to/godot
```

Get the source from https://ffmpeg.org/releases/ and verify the upstream release
signature before using it for a public release. The helper uses system
VideoToolbox and enables no external codec library, GPL code or nonfree code.
It records its configure command in `build/ffmpeg/build-info.json`.

On Windows/Linux, `python scripts/desktop_sdk.py` builds a checksum-pinned LGPL
FFmpeg 9.0.1 shared SDK with NVENC, AMF and QSV. See [desktop releases](desktop-releases.md)
for prerequisites and the complete `scripts/desktop_release.py` build/check/package flow.
Alternatively, supply an LGPL FFmpeg 9 SDK with the hardware wrappers needed
for the target GPU (NVENC, AMF or QSV). A wrapper being compiled in does not prove
that a particular device or driver can open an encoder. The POSIX SDK helper can
also prepare Linux libraries for unit tests; add the required hardware SDK headers
and `--extra-configure=...` flags to enable production encoding there.

```sh
python3 scripts/build.py --ffmpeg-dir /path/to/lgpl-ffmpeg-9 --godot /path/to/godot
```

The build rejects a GPL/nonfree FFmpeg SDK and mismatched libavcodec major versions.
Homebrew's default FFmpeg and BtbN's **gpl-shared** distributions are unsuitable.
Include the SDK's license texts under `licenses/`. Native builds use `pug extension
build --platform <host>:<arch> --with-engine <editor>`; the wrapper handles copying
libraries, macOS install names/ad-hoc signatures and manifest generation.
Ad-hoc signatures are for local validation; public macOS signing/notarization is
a separate release step requiring the publisher's signing identity.

The installed addon is under `addons/game_stream/`. Its generated manifest
supports editor/debug and release, and lists FFmpeg libraries in `[dependencies]`
for Godot exports. macOS export dependencies target `Contents/Frameworks`.
For Linux/Windows SDKs, inspect transitive runtime dependencies on a clean host
before releasing; this script is not a replacement for platform export testing.

## Tests

```sh
python3 -m unittest discover -s tests -v
cd native/game_stream
FFMPEG_DIR=/path/to/sdk cargo test --locked --release
FFMPEG_DIR=/path/to/sdk cargo clippy --locked --all-targets -- -D warnings
```

On Linux, include the SDK `lib/` directory in `LD_LIBRARY_PATH` when testing.
On macOS, set `DYLD_FALLBACK_LIBRARY_PATH` to the SDK `lib/` directory for Cargo tests.
Keep the same `FFMPEG_DIR` and `PKG_CONFIG_PATH` for builds, tests and Clippy.
Use `cargo fmt --check` for Rust formatting.

For a fresh headless editor import check, use:

```sh
python3 tests/smoke_editor.py --godot /path/to/godot
```

The check allows editor documentation to finish before shutdown. Immediate
headless `--import`/`--quit` can hit upstream Godot issue #111645; see
[validation notes](https://github.com/godotengine/godot/issues/111645).

After building the native addon, open the root project with official Godot and
run the demo. Connect mirctl, confirm moving video, press the arrow keys and click
in the stream. Check that the demo's click counter changes. Repeat in an exported
game, and on a host without the SDK on its library search path.

## mirctl

```sh
python3 clients/mirctl/scripts/build.py
# If all Conan dependencies are cached:
python3 clients/mirctl/scripts/build.py --offline
python3 clients/mirctl/scripts/build.py --lockfile /path/to/conan.lock
ctest --test-dir clients/mirctl/build/app/Release --output-on-failure
```

The client builds with CMake, Conan 2 and a C11 compiler. Its FFmpeg 9.0.1 build
is decoder-only and disables x264, x265, postproc and nonfree codec dependencies.
FFmpeg, SDL3 and the Windows CRT are statically linked. Keep the corresponding
Conan recipe revisions, complete dependency sources, licenses and build instructions
with client binary releases so recipients can rebuild/relink modified libraries.
The build records its resolved graph and lockfile under `clients/mirctl/build/`.
The release packaging step collects the exact dependency sources, recipes,
static libraries, application objects and notices. See
[client rebuilding](client-rebuilding.md). The local `dist/mirctl` executable
alone is not a complete public distribution.

## Addon ZIP

```sh
cargo vendor --locked --manifest-path native/game_stream/Cargo.toml build/rust-vendor
python3 scripts/package.py --ffmpeg-source /exact/source/used/for/sdk --client
```

Commit the final source before packaging. The original v0.1.0 package names are
reserved for macOS arm64. Use `--supplemental` for Windows/Linux builds so their
source and dependency archives do not replace existing release materials.
Package one platform per invocation. It produces an addon ZIP, a client
ZIP, first-party/FFmpeg/Rust/client-dependency source materials, a build record
and checksums under `dist/`. Publish the six ZIPs, `release-build.json` and
`SHA256SUMS` together. The first-party source ZIP includes only tracked files
from the recorded source commit. Client dependency downloads are verified
against the SHA-256 values in the resolved Conan recipes.

The package release target is macOS 26.0+, Godot 4.6.2 and Forward+ with Metal.
See the [release notes](https://github.com/yearsyan/godot-game-stream/blob/v0.1.0/docs/releases/0.1.0.md)
for validated scope and pending work.
