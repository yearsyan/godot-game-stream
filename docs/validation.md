# Validation — 2026-09-06

Host: macOS, Apple M4 Pro / arm64, Metal Forward+.

| Check | Result |
| --- | --- |
| Rust release tests | 27 passed, including TCP/UDP and shared C/Rust fixtures |
| Rust Clippy | Passed with `-D warnings` (release, all targets) |
| Rust formatting | Passed |
| Python distribution checks | 3 passed |
| mirctl native build and CTest | Built; GSI1 fixtures passed |
| Official Godot 4.6.2 standard fresh editor import/exit | Passed with paced editor startup; no script/engine errors |
| Official Godot 4.6.2 game smoke | 259 H.264 frames decoded at 1280×720 from the final repository location; remote mouse input passed; clean exit |
| Official Godot 4.7.2 .NET game smoke | 196 H.264 frames decoded; remote mouse input passed |

The SDK was built from FFmpeg 9.0.1 sources with only LGPL components and native
VideoToolbox encoding. Every bundled FFmpeg library reported LGPL. No external
codec library was needed. Forced VideoToolbox constant bitrate was removed after
the M4 session rejected it; the target bitrate is now used with ABR.

## Headless editor import caveat

Fresh headless imports with immediate `--import`/`--quit`, or only ten unpaced
editor frames, crashed during shutdown on both 4.6.2 standard and 4.7.2 .NET.
The stack enters `EditorHelp::_gen_extensions_docs` and `DocTools::generate`
from `Main::cleanup`. This matches the confirmed upstream issue
[godotengine/godot#111645](https://github.com/godotengine/godot/issues/111645).
It is not evidence that extra filesystem permissions fix the crash.

On 4.6.2 standard, a fresh editor run with `--max-fps 30 --quit-after 300`
completed import and exited successfully. The regression check below uses this
ten-second startup window rather than immediate shutdown. It is a tested
workaround on this host, not an engine fix or a guarantee for slower hosts.
The addon icon is now decoded directly from SVG source, so plugin activation
does not depend on its first resource import having completed.
The paced editor check has not been repeated on 4.7.2 .NET.

```sh
python3 tests/smoke_editor.py --godot /path/to/official/godot
```

## Remaining release validation

- Windows/Linux builds and driver combinations have not been validated here.
- Exported-game runtime dependency placement and public macOS signing/notarization
  still need release validation. The current addon build is locally ad-hoc signed.
- Client executable is a local build. Public client releases also need complete
  Conan dependency source/license/rebuild materials as described in building.md.

Reproduce the video/input check with:

```sh
python3 tests/smoke_godot.py --godot /path/to/official/godot
```

The test owns its Godot process, connects only to loopback, sends fixture input,
uses ffprobe to decode/count the received video, and waits for the demo to stop.
The Godot window is expected; `--headless` cannot validate GPU capture.
