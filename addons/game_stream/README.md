# Godot Game Stream

LGPL-2.1-or-later · v0.1.0 · Windows x86_64 / Linux x86_64 / macOS arm64

Use the combined desktop archive or the archive matching your operating system.
The combined archive includes a runnable `example/demo.tscn` scene and exact
native source links in `SOURCES.md`. The addon targets Godot 4.6.2
standard precision and Forward+. Windows/Linux use NVENC, AMF or QSV; macOS
26.0+ arm64 uses VideoToolbox. H.264 over TCP is the default. Windows/Linux
streaming was validated by the maintainer in GachaGameProducer; the standalone
desktop artifacts undergo native builds, tests and dependency loading checks.
See `release-notes.md` and `desktop-releases.md` for requirements and scope.

Fresh installation and relocated release-export dependency checks pass on all
three platforms. A macOS export also passes H.264 streaming and remote mouse
input checks. Windows/Linux exported hardware streaming needs validation with
the target GPU; the hosted export checks exercise native library loading.

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
