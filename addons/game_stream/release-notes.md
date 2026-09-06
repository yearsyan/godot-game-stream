# Godot Game Stream 0.1.0

Initial standalone release of the LGPL-2.1-or-later Godot addon and mirctl client.

Windows x86_64 and Linux x86_64 addon/client packages are available as supplemental
release assets. Their source commit, dependency requirements and validation scope
are recorded in `release-build-<platform>.json` and `desktop-releases.md`. The
maintainer has validated both platforms in GachaGameProducer. The original macOS
assets and tag are unchanged; the configuration table below describes those assets.

## Supported configuration

| Component | Release target |
| --- | --- |
| Operating system | macOS 26.0+; tested on macOS 26.6.2 |
| CPU | Apple Silicon / arm64 |
| Godot | Official Godot 4.6.2, standard precision |
| Renderer | Forward+ with Metal |
| Video | H.264, VideoToolbox hardware encoding, TCP |
| Client | Bundled mirctl for macOS arm64 |
| Input | GSI1 keyboard and mouse over the TCP connection |

The binary deployment target is macOS 26.0 because its FFmpeg and client
dependencies were built with that minimum. Older macOS releases require a
separate dependency rebuild and validation. Intel Macs, other
Godot versions, the Mobile renderer, HEVC/AV1 and UDP are outside this release's
validated scope, even where the source contains implementations.

For Windows/Linux, download `godot-game-stream-0.1.0-<platform>.zip` and
`mirctl-0.1.0-<platform>.zip`, where `<platform>` is `windows-x86_64` or `linux-x86_64`.
Use the corresponding platform-specific source and dependency archives when rebuilding.

## Downloads

- `godot-game-stream-0.1.0-macos-arm64.zip`: ready-to-install `addons/game_stream/`
  with the native extension, FFmpeg shared libraries and dependency notices.
- `mirctl-0.1.0-macos-arm64.zip`: standalone viewer, readme and dependency notices.
- `godot-game-stream-0.1.0-source.zip`: matching first-party source and build tools.
- `ffmpeg-source.zip`: exact FFmpeg source for the addon's shared SDK.
- `rust-dependencies.zip`: locked Rust dependency sources.
- `mirctl-0.1.0-dependencies.zip`: client dependency sources, Conan recipes,
  revisions/options, static libraries and rebuilding instructions.
- `release-build.json` and `SHA256SUMS`: source revision, build metadata and checksums.

Extract the addon's `addons/` directory into a Godot project, enable the plugin,
add a GameStreamHost node and enable Auto Start. Run the game and connect with
`./mirctl 127.0.0.1:20831`. Both sides default to H.264.

## Changes

- Extract the native extension and C viewer into one independent repository.
- Add an inspector-configured GameStreamHost and a minimal GDScript demo.
- Bundle LGPL FFmpeg libraries and preserve dependency/source materials.
- Remove the GPL x264 fallback; use hardware encoding.
- Use VideoToolbox's target bitrate without forcing unsupported constant bitrate.
- Add shared C/Rust input fixtures, packaging checks and CI.
- Use typed byte chunks to satisfy current Clippy checks.

## Limits and remaining validation

The release is for editor-launched games. Exported-game dependency placement and
runtime validation remain pending. The macOS binaries are ad-hoc signed and are
not Developer ID signed or notarized. These are not claims of an App Store or
Godot Asset Store approval; no store submission is included in this release.

Immediate headless editor import/shutdown can hit Godot issue #111645. The
repository's editor smoke check uses a paced startup to avoid that observed
shutdown race. Headless and Compatibility renderers cannot capture video.

There is no audio, codec negotiation, authentication or encryption. The listener
defaults to loopback. Use a trusted tunnel for remote access, and match the
client's codec to the host when experimenting with other codecs.

This software uses FFmpeg under LGPL-2.1-or-later. Corresponding FFmpeg, Rust and
client dependency sources are provided with this release. Dependencies retain
their own licenses; consult the notices included in each download.
