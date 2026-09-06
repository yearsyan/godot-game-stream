# mirctl

LGPL-2.1-or-later desktop client for Godot Game Stream. Pure C11, FFmpeg 9.0.1,
SDL3 3.4.14. Receives raw H.264/HEVC/AV1 over TCP and sends GSI1 keyboard/mouse
events back over the same connection. No audio, recording or Android device layer.

```sh
python3 scripts/build.py
python3 scripts/build.py --offline   # previously cached Conan dependencies
ctest --test-dir build/app/Release --output-on-failure
dist/mirctl 127.0.0.1:20831
dist/mirctl --codec=hevc 192.168.1.20:20831
dist/mirctl --codec=av1 192.168.1.20:20831
dist/mirctl --no-control 127.0.0.1:20831
```

Windows produces `dist/mirctl.exe`. The build script initializes MSVC when needed.
Requires Python 3, CMake, Conan 2 and a C11 compiler. FFmpeg and SDL3 are linked
statically; FFmpeg is decoder-only with no GPL/nonfree codec dependencies. The
matching dependency source/license/rebuild materials are required with public
binary releases; see the root `THIRD_PARTY.md` and `docs/building.md`.

H.264 is the default. Match the Godot host's codec explicitly when using
HEVC/AV1; there is no negotiation. Supports hardware decoding with software fallback,
YUV420P/NV12/P010, latest-frame display, letterboxing, mouse coordinate mapping and
dynamic video sizes. The first frame sets the window to native video dimensions,
limited by the available desktop space.

The transport is TCP-only and offers no authentication or encryption. Control
writes currently run synchronously on the event thread. There are no IME/text or
gamepad messages. See [the wire protocol](../../docs/protocol.md) and
[architecture notes](ARCHITECTURE.md) for implementation and build details.

Formatting: `python3 scripts/format.py --check`.
