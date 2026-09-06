# Source provenance

Extracted with the copyright holder's authorization on 2026-09-06.

- Native: `GachaGameProducer`, revision
  `77ae9f80baf0e40bbb51fec9d1da0816128e1695`, `extensions/game_stream/`.
- Client: `mirctl`, revision `973f903` (AV1 VideoToolbox support).

The independent repository adopts LGPL-2.1-or-later for first-party material.
It excludes the original games, assets, custom engine modules, local configuration,
credentials, caches, generated binaries and unrelated commit history.

Intentional native changes: remove the GPL x264 fallback and its backend-specific
test; default to H.264; add explicit per-host codec/input configuration; reject
non-LGPL libavcodec at startup. The existing raw video and GSI1 protocol remain
compatible with mirctl. The old project-specific C# startup task is replaced by
a reusable GDScript host that does not change the game window's resolution.

Validation also found a VideoToolbox session that rejected forced constant bitrate.
The standalone build uses the configured target bitrate with ABR on that backend.
