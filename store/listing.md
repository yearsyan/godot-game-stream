# Godot Asset Store listing

## Identity

- Publisher: yearsyan
- Publisher slug: yearsyan
- Asset name: Godot Game Stream
- Asset slug: godot-game-stream
- Type: Addon
- Price: Free
- License: LGPL-2.1-or-later
- Version: 0.1.0
- Minimum Godot version: 4.6.2, standard precision
- Source: https://github.com/yearsyan/godot-game-stream
- Suggested tags: GDExtension, Streaming, Remote Control, Networking, Tool

## Summary

Stream a Godot viewport to the mirctl desktop viewer with hardware video encoding and remote keyboard and mouse input.

## Detailed description

Godot Game Stream captures the running game's root viewport and streams it to
mirctl, a lightweight desktop viewer from the same open-source project. Control
the game from the viewer with keyboard and mouse input, or connect in view-only
mode. It is useful for remote playtesting and inspecting a game on another
desktop over a trusted connection.

### Features

- Inspector-configured GameStreamHost node and a small runnable example.
- H.264 over TCP by default, with HEVC and AV1 options on compatible hardware.
- In-process GPU capture and encoding through a Rust GDExtension and LGPL FFmpeg.
- Windows and Linux hardware encoding through NVENC, AMF or QSV; macOS through VideoToolbox.
- One installation ZIP with Windows x86_64, Linux x86_64 and macOS arm64 libraries.
- Separate mirctl downloads for all three platforms, with corresponding sources and rebuilding materials.
- No custom Godot engine or .NET requirement.

### Quick start

1. Install the addon into your project and enable Godot Game Stream in Project Settings > Plugins.
2. Add a GameStreamHost node, enable Auto Start, and run a scene with the Forward+ renderer.
3. Download and extract mirctl for your operating system from the GitHub release.
4. Run `mirctl 127.0.0.1:20831`. Both sides default to H.264.

To try the included example, open `addons/game_stream/example/demo.tscn` and run
that scene. Move with the arrow keys and click in the viewer to change color.
For another codec, select it on the host and pass the matching `--codec=hevc` or
`--codec=av1` option to mirctl. Automatic codec selection does not negotiate with
the client.

[Download mirctl and matching source materials](https://github.com/yearsyan/godot-game-stream/releases/tag/v0.1.0)

### Requirements and scope

Godot 4.6.2 standard precision and Forward+ are the tested baseline. A rendered
viewport and a working hardware encoder are required; there is no software x264
fallback. Headless rendering, the Compatibility renderer, mobile and Web are
outside this release's supported scope.

- Windows: Windows 10 or later, x86_64. A supported GPU and its encoding drivers
  are required. The addon may require the Microsoft Visual C++ 2015-2022 x64
  Redistributable. The bundled NVIDIA wrapper targets SDK 13 and needs R570 or
  newer drivers. RTX 3080 supports HEVC encoding but not AV1 encoding.
- Linux: x86_64, glibc 2.35 or later; system libva, libva-drm, libdrm and GPU
  drivers. The mirctl viewer uses X11 or Xwayland. Intel QSV additionally needs a
  compatible system media driver.
- macOS: macOS 26.0 or later on Apple Silicon, using Forward+ with Metal and
  VideoToolbox. H.264 is the validated configuration. Binaries are ad-hoc signed.

Windows and Linux streaming have been validated by the maintainer in
GachaGameProducer. Native CI checks builds, dependency loading and protocol
fixtures. Hosted CI does not test hardware encoding. See the repository's
validation records for the exact standalone package and export checks.

### Connection behavior

Streaming is opt-in: Auto Start defaults to off and the listener defaults to
127.0.0.1. Allow Input controls whether the client can inject keyboard and mouse
events. For remote access, use a trusted network or authenticated tunnel. The
stream protocol itself has no authentication or encryption.

One stream is supported. There is no audio or codec negotiation. mirctl uses
TCP; the native UDP sender is experimental and requires a different receiver.

### License and sources

First-party code and assets use LGPL-2.1-or-later. Dependencies retain their own
licenses. Each download includes notices, and the release provides matching
first-party sources, FFmpeg sources, other dependency materials and client
relinking inputs. The addon's SOURCES.md and distribution.json identify each
platform's exact native build.

## AI usage disclosure

AI assistance was used for code, documentation, packaging, tests and listing
materials. The maintainer reports native Windows and Linux application testing;
automated checks and macOS streaming checks are recorded in the repository.
The demonstration image is decoded from an actual H.264 stream from a release
export of the bundled Godot example. The thumbnail is generated from editable
vector artwork.

## Version changelog

Initial standalone release of Godot Game Stream and mirctl. Includes hardware
video encoding, TCP streaming, optional remote keyboard and mouse input, an
Inspector host node, a runnable example and desktop native libraries. The store
ZIP combines the existing platform release binaries with one extension manifest,
platform notices and exact source links.

## Reviewer notes

The addon installation ZIP contains only addons/game_stream/. The mirctl viewer
is downloaded separately from the release linked above. Hardware encoding needs
a supported GPU and drivers. This is a Godot project addon, not a replacement
engine. The source repository intentionally excludes generated native binaries;
use the uploaded desktop ZIP to install the ready-to-run addon.

The original v0.1.0 tag and platform artifacts remain unchanged. The combined
store ZIP records the individual native source commits in distribution.json.
