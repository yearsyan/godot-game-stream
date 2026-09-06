# game_stream native extension

Rust GDExtension for Godot 4.6+, licensed LGPL-2.1-or-later.

From the repository root run `python3 scripts/build.py --ffmpeg-dir <SDK>
--godot <editor>`. This invokes `pug extension build`, bundles LGPL FFmpeg shared
libraries and writes the addon's `.gdextension` manifest.

`GameStream` is an engine singleton with `listen`, `listen_with_options`,
`listen_configured`, `stop`, `is_listening` and `stats` methods. New integrations
should use `addons/game_stream/host.gd` or `listen_configured`, whose explicit
codec and input settings are not overridden by environment variables.

The legacy `listen` methods honor `GAME_STREAM_CODEC` and `GAME_STREAM_INPUT`.
Both legacy defaults and the addon default to H.264. No game-specific startup
framework, engine patch or C# assembly is required.

Unit tests: `FFMPEG_DIR=<SDK> cargo test --locked --release`.
The x264-only encoding test was removed with the GPL backend; actual encoding is
validated through the Godot demo on a host with an available hardware encoder.
Shared GSI1 fixtures are under `../../protocol/fixtures`.
