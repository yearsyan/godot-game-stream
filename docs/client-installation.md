# mirctl 0.1.0

This package contains the Godot Game Stream desktop viewer. Windows x86_64 and
Linux x86_64 packages have separate requirements in `desktop-releases.md`.
Use `mirctl.exe` in Windows PowerShell and `./mirctl` on Linux/macOS.

The macOS arm64 package requires
macOS 26.0+ on Apple Silicon and was tested on macOS 26.6.2. FFmpeg, SDL3 and
dav1d are linked statically; no Homebrew packages or separate codec DLLs are
needed to run it. The executable is ad-hoc signed and is not notarized.

## Run

Extract the ZIP, open a terminal in the extracted directory and run:

```sh
./mirctl --version
./mirctl 127.0.0.1:20831
```

Start the Godot game first, using the companion addon configured for H.264.
The supported host is Godot 4.6.2 with Forward+ (Metal on macOS). Close the viewer window
to disconnect. Use `./mirctl --help` for connection and input options.

The protocol has no authentication or encryption. Use loopback or a trusted
tunnel for remote access. There is no audio. See `release-notes.md` for scope
and remaining validation.

## License and rebuilding

mirctl is LGPL-2.1-or-later; see `LICENSE` and `COPYING.GPLv2`. It uses FFmpeg
under LGPL-2.1-or-later, SDL3 under the zlib license and dav1d under BSD-2-Clause.
Their notices are in `THIRD_PARTY/`.

The matching source, dependency archives, application objects and rebuilding
instructions are available in the same
[GitHub release](https://github.com/yearsyan/godot-game-stream/releases/tag/v0.1.0).
Download `godot-game-stream-0.1.0-source.zip` and
`mirctl-0.1.0-dependencies.zip` to rebuild or relink this executable.
For Windows/Linux, use the platform-specific first-party source and client
dependency archives instead; their filenames are listed in `desktop-releases.md`.
