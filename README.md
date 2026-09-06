# Godot Game Stream

Stream a running Godot game's root viewport to **mirctl**, a lightweight desktop
viewer with keyboard and mouse control. The Godot addon and its client are
maintained together in this repository.

**License: LGPL-2.1-or-later.** Copyright (C) 2026 yearsyan and contributors.
Third-party components retain their own licenses; see [THIRD_PARTY.md](THIRD_PARTY.md).

## Features

- Rust GDExtension using Godot's public APIs; no custom engine or .NET required.
- GPU resize/color conversion, asynchronous readback, hardware video encoding.
- H.264 by default; explicit HEVC, AV1 or hardware capability probing (`auto`).
- TCP video with GSI1 keyboard/mouse input on the same connection.
- Latest-frame queues, keyframe recovery and runtime statistics.
- Native C client with FFmpeg decoding and SDL3 rendering.

The **v0.1.0 binary release** includes **Windows x86_64, Linux x86_64 and macOS arm64**.
Windows/Linux require a supported NVIDIA, AMD or Intel hardware encoder and its driver.
The macOS build targets **macOS 26.0+ on Apple Silicon (arm64)**,
**Godot 4.6.2 standard precision**, and **Forward+ with Metal**. H.264 over TCP
with VideoToolbox encoding is the supported configuration. It was tested on
macOS 26.6.2; older macOS versions need separately targeted builds.
See [release notes](docs/releases/0.1.0.md) and
[validation results and the headless import caveat](docs/validation.md).

The maintainer has validated Windows/Linux streaming in GachaGameProducer;
the supplemental binaries also undergo native CI builds and loader/unit checks.
See [desktop release details](docs/desktop-releases.md) for requirements and provenance.
Other Godot versions and renderers remain experimental for this release.
The source also supports the RenderingDevice Mobile renderer;
Compatibility and `--headless` cannot capture.
Desktop backends are NVENC/AMF/QSV on Windows/Linux and VideoToolbox on macOS.
The LGPL distribution has **no x264 software fallback**: a working hardware encoder
is required. macOS encoding supports H.264/HEVC; AV1 requires another supported
hardware backend. There is no audio, browser client, authentication or encryption.
The default listener is loopback; use a trusted tunnel for remote access.

## Install and run

1. Download `godot-game-stream-0.1.0-desktop.zip` for all supported desktop
   platforms, or a platform-specific `godot-game-stream-0.1.0-<platform>.zip`, from
   [Releases](https://github.com/yearsyan/godot-game-stream/releases/tag/v0.1.0)
   and extract its `addons/`
   directory into your Godot project. A source checkout requires a native build.
2. Enable **Godot Game Stream** in Project Settings → Plugins.
3. Add a **GameStreamHost** node to your scene and enable **Auto Start**.
4. Download and extract `mirctl-0.1.0-<platform>.zip` from the same release.
   Run the game, then run `./mirctl 127.0.0.1:20831` on the same machine
   (`.\mirctl.exe 127.0.0.1:20831` in Windows PowerShell).

Both sides default to H.264. If you select HEVC or AV1 in the host inspector,
pass the same `--codec=hevc` or `--codec=av1` to mirctl. `auto` does not negotiate
with the client; inspect `get_stats()["active_codec"]` and select that codec.

The host captures the root viewport without resizing the game's window. Requested
dimensions are clamped to the source and rounded down to even sizes. Only one host
can own the singleton stream at a time. Streaming never starts inside the editor.

The combined desktop archive includes `addons/game_stream/example/demo.tscn`.
Open and run that scene to try streaming without setting up another scene.
See [store package validation](store/validation.md) for fresh-install and
relocated export checks, including actual macOS export streaming.

For direct script control, attach `addons/game_stream/host.gd` to a Node and call
`start_stream()`, `stop_stream()` and `get_stats()`. `start_stream()` returning `OK`
means startup was accepted; encoding initializes on the first captured frame.
Use the statistics and Godot log to check encoder readiness or failure.

## Develop

Open the root `project.godot` after building the extension. The included demo has
moving shapes and a mouse-click counter for validating video and remote input.

```sh
# LGPL FFmpeg 9 shared SDK, Rust and pug must be installed.
python3 scripts/build.py --ffmpeg-dir /path/to/ffmpeg-sdk --godot /path/to/godot
python3 clients/mirctl/scripts/build.py
```

See [building and packaging](docs/building.md), [wire protocol](docs/protocol.md)
and [publication](docs/publishing.md) for details. Native builds use `pug`; end
users installing the finished addon do not need a compiler or pug.

## Repository

| Directory | Purpose |
| --- | --- |
| `addons/game_stream/` | Godot plugin, host node and generated platform libraries |
| `native/game_stream/` | Rust capture, encoding, transport and input implementation |
| `clients/mirctl/` | Standalone C viewer and its own CMake/Conan build |
| `examples/` | Minimal GDScript project content |
| `protocol/fixtures/` | Shared binary protocol fixtures used by Rust and C tests |
| `scripts/` | SDK preparation, pug build and addon packaging |

Version 0.1.0 is the initial standalone extraction. See
[PROVENANCE.md](PROVENANCE.md) for the source revisions and intentional changes.
