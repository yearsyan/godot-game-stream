# Third-party components

First-party source, scripts, examples and artwork: **LGPL-2.1-or-later**.
The root LICENSE contains the complete LGPL text and copyright notice.
COPYING.GPLv2 is included because LGPL 2.1 refers to that license.

Dependencies are not relicensed by this repository. Cargo.lock and the Conan
configuration identify dependency versions; retain upstream notices and exact
dependency source/build records when distributing compiled artifacts.

| Component | Use | Source |
| --- | --- | --- |
| godot / gdext Rust bindings (MPL-2.0) | Native Godot ABI and generated bindings | https://github.com/godot-rust/gdext |
| ffmpeg-next / ffmpeg-sys-next (WTFPL) | Rust FFmpeg wrappers | https://github.com/zmwangx/rust-ffmpeg |
| FFmpeg 9 | Native encoding and client decoding | https://ffmpeg.org/ |
| SDL3 | mirctl window, rendering and input | https://github.com/libsdl-org/SDL |
| dav1d | mirctl AV1 software decoding | https://code.videolan.org/videolan/dav1d |

The provided native SDK helper selects LGPL FFmpeg configuration with no x264,
x265, GPL or nonfree components. Native build and runtime checks reject a
libavcodec whose reported license is not LGPL. Third-party libraries added to a
custom SDK need a separate dependency/license review and must be packaged too.

The client statically links FFmpeg/SDL/dav1d. Its public binary release must carry
their notices, corresponding sources, exact recipes/options and the complete
rebuild/relink instructions. Keeping only an executable and a link to this
repository is insufficient to document its dependency build.
