# Godot Game Stream

LGPL-2.1-or-later · v0.1.0 · macOS arm64

This binary release targets macOS 26.0+, Godot 4.6.2 standard precision and
Forward+ with Metal. H.264 over TCP with VideoToolbox encoding is the supported
configuration. The release was tested on macOS 26.6.2. Other platforms, renderers,
Godot versions and codecs are experimental. See `release-notes.md`.

Install this entire folder under `addons/`, enable **Godot Game Stream** in
Project Settings → Plugins, and add a **GameStreamHost** node to a scene.
Enable its **Auto Start** setting and run the game. Launch the mirctl desktop
client with `mirctl 127.0.0.1:20831`.

H.264 is the default on both sides. For HEVC/AV1, set the host's **Codec** and
use the matching `mirctl --codec=hevc` / `--codec=av1` argument. Automatic codec
selection requires manually matching the codec reported by `get_stats()`.

The native extension and FFmpeg runtime libraries in `bin/` are required.
Source checkouts do not contain these generated files; build with the root
repository's `scripts/build.py`. No custom Godot engine or .NET is required.

Requires a supported hardware encoder and a rendered root
viewport. Compatibility, headless rendering, audio and mobile exports are not
supported by this release. There is no x264 software fallback. TCP is supported
by mirctl; the native UDP sender is experimental and needs another client.

**Auto Start** defaults to off; the default address is `127.0.0.1`.
Connections can view the game and inject input when **Allow Input** is enabled.
Use trusted networks/tunnels; the protocol provides no authentication or TLS.

Attach `host.gd` to a Node for script integration:

```gdscript
var result: Error = $StreamHost.start_stream()
if result != OK:
    push_error(error_string(result))
print($StreamHost.get_stats())
# On demand:
$StreamHost.stop_stream()
```

Startup is asynchronous: `OK` is acceptance, not proof of a first encoded frame.
The host stops its stream when it exits the tree. Only one stream is supported.

Copyright (C) 2026 yearsyan and contributors. See `LICENSE`, `COPYING.GPLv2`
and `THIRD_PARTY/` for license texts and FFmpeg build information. The matching
first-party and FFmpeg source ZIPs must accompany binary releases.
