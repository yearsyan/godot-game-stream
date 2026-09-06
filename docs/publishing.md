# Publication

Target the official [Godot Asset Store](https://store.godotengine.org/) as an
**Addon**, with the standalone source repository linked in its metadata.
The legacy Asset Library is deprecated; check whether it still accepts submissions
if compatibility with its older in-editor frontend is desired.

The Godot upload contains `addons/game_stream/` only. mirctl is a separate
platform-specific download in the same repository's Releases. Never upload the
whole monorepo as the plugin installation payload.

The prepared store release uses the combined desktop ZIP assembled by
`python3 scripts/package_store.py`. It contains three platform library entries,
their runtime dependencies, notices, exact source links and a runnable example.
The input archives and checksums are pinned in `store/package-inputs.json`.
English listing fields and AI disclosure are in `store/listing.md`; thumbnail,
icon and an actual streamed screenshot are in `store/media/`.

The tested minimum is Godot 4.6.2 standard precision. The API target alone is not
proof of compatibility. See `store/validation.md` for fresh installation,
relocated export loading and the exact hardware streaming scope. Publish only
the platforms/renderers covered by these claims.

The LGPL-2.1-or-later declaration applies to first-party source and assets.
Keep the exact third-party license and source notices with each binary package.
Source ZIPs, build commands and changes must correspond to those binaries.
Do not describe an unverified GPL/nonfree FFmpeg build as an LGPL distribution.

The source repository is [yearsyan/godot-game-stream](https://github.com/yearsyan/godot-game-stream).
GitHub Releases and the Asset Store listing are separate publication steps.
Configure release URLs and signing identities when publishing binary packages.

References:
- https://godotengine.org/article/introducing-the-godot-asset-store/
- https://docs.godotengine.org/en/latest/community/asset_store/submitting_to_asset_store.html
- https://ffmpeg.org/legal.html
