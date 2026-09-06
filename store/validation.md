# Store package validation

The store package combines the original macOS arm64 binary and the supplemental
Windows/Linux x86_64 binaries from v0.1.0. The native bytes are preserved; the
manifest, example and shared documentation are assembled together. Input
archive SHA-256 values and native source commits are in `package-inputs.json`.

## Results

| Check | Windows x86_64 | Linux x86_64 | macOS arm64 |
| --- | --- | --- | --- |
| Clean official Godot 4.6.2 editor import and exit | Passed | Passed | Passed |
| Release export with bundled native dependencies | Passed | Passed | Passed |
| Run after relocating the export and hiding the source project | Passed | Passed | Passed |
| Load GameStream without a development FFmpeg SDK in the library path | Passed | Passed | Passed |
| Actual exported H.264 stream and remote mouse input | Requires target GPU | Requires target GPU | Passed, 271 frames at 1280 x 720 |

Windows and Linux export checks run on native Windows 2022 and Ubuntu 22.04
GitHub runners. They exercise the exported singleton and dependency loading in
headless mode, without claiming hardware encoder validation. The maintainer
separately reports Windows/Linux streaming tests in GachaGameProducer.

The macOS check runs on macOS 26.6.2, Apple M4 Pro, Forward+/Metal and VideoToolbox.
It launches the exported game with a real display, independently decodes the
received H.264 stream, delivers a GSI1 mouse press/release and checks clean exit.
The official macOS template contains a universal engine; the bundled addon and
the validated host are arm64. This is not a claim of Intel Mac support.

Initial native export evidence:
[Windows and Linux workflow](https://github.com/yearsyan/godot-game-stream/actions/runs/34043247426).
Final package checksums and validation records accompany the combined release.

## Reproduce

```sh
python3 scripts/package_store.py
python3 scripts/validate_store.py
```

The second command downloads checksum-pinned official editor/templates and
creates a fresh project. Use a clean `build/store-validation/` directory for each
run. Godot uses a paced editor import to avoid the known immediate-shutdown issue
documented in `docs/validation.md`.

On a Mac with the release's supported GPU/display configuration, run:

```sh
python3 tests/check_store_export.py \
  --godot build/store-tools/editor/Godot.app/Contents/MacOS/Godot \
  --templates build/store-tools/templates \
  --package dist/store/godot-game-stream-0.1.0-desktop.zip \
  --output build/store-export-macos --stream
```

The test enables ETC2/ASTC import and uses the official universal macOS template.
Godot exports and ad-hoc signs the application. No Developer ID signing or
notarization is performed.

All checks keep the listener on loopback. No installed engine, user game project,
GPU driver configuration or system security setting is changed by these checks.
