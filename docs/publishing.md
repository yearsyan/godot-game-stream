# Publication

Target the official [Godot Asset Store](https://store.godotengine.org/) as an
**Addon**, with the standalone source repository linked in its metadata.
The legacy Asset Library is deprecated; check whether it still accepts submissions
if compatibility with its older in-editor frontend is desired.

The Godot upload contains `addons/game_stream/` only. mirctl is a separate
platform-specific download in the same repository's Releases. Never upload the
whole monorepo as the plugin installation payload.

Before submitting a version, validate official Godot and exported games for every
claimed platform/renderer, include license texts and dependency/source materials,
and prepare an English description, icon, screenshot/video and changelog.
Set the minimum version to 4.6 only after testing that version; the current API
target alone is not proof of runtime compatibility. Record AI usage in the store's
required disclosure field when applicable.

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
