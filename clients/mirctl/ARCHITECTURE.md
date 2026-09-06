# mirctl architecture

mirctl receives raw H.264/HEVC Annex-B or AV1 low-overhead OBU over TCP,
decodes with FFmpeg, and renders with SDL3. Keyboard and mouse events travel
back to the game as GSI1 messages on the same TCP connection. The client has
no audio, recording, device management or scrcpy Android protocol layer.

## Components

```text
                         +-- GSI1 keyboard/mouse ----------------+
                         |                                      v
SDL main thread --> control.c --> transport.h abstraction --> TCP socket
      ^                                    |                    |
      |                                    |                    | H.264/HEVC
      |                                    |                    | / AV1 OBU
      |                                    |                    v
 renderer.c <-- latest-frame mailbox <-- decoder.c <-- receive worker
   SDL3              capacity = 1          FFmpeg / dav1d
```

- `transport.c` provides complete writes, interruptible reads and portable TCP
  sockets behind the narrow `transport.h` interface.
- `decoder.c` uses FFmpeg parsers to reconstruct H.264/HEVC access units. It
  splits AV1 temporal units at temporal delimiter OBU boundaries itself. Each
  codec tries hardware decoding supported by both the FFmpeg build and device.
  Packets are retained until the first frame: if hardware format negotiation,
  packet submission, frame reception or GPU readback fails, the decoder reopens
  in software and replays them to preserve sequence headers and the first
  keyframe. AV1 software fallback uses libdav1d because FFmpeg's native AV1
  decoder requires hardware acceleration.
- `renderer.c` supports SDL3 IYUV, NV12 and P010 textures, letterboxing and
  window-to-video coordinate conversion. P010 handles 10-bit hardware frames.
  Textures are recreated for the first frame or a size/pixel-format change.
  The initial window follows the video's native pixel dimensions, accounting
  for display density and shrinking proportionally to fit the usable desktop.
- `control.c` maps SDL input to Godot enums and serializes GSI1 frames in
  little-endian order.
- `core.c` receives and decodes on a worker thread while SDL renders on the
  main thread. A single-frame mailbox discards stale decoded frames to limit
  display latency.

The core does not depend on socket types. A future WebSocket implementation
could use an established C library behind `transport.h`, exposing incoming
binary messages as continuous bytes and mapping each `write_all` call to one
GSI1 binary message. HTTP/WebSocket handshakes and framing belong in that
transport backend.

## Build details

The build requires CMake, Conan 2, Python 3 and a C11 compiler. Conan supplies
FFmpeg 9.0.1 and SDL3 3.4.14 as static libraries. FFmpeg includes avcodec/avutil,
H.264/HEVC/AV1 decoders, required hardware accelerators, H.264/HEVC parsers and
the libdav1d wrapper. Encoders, audio, containers, filters, devices, network
protocols and other external codecs are disabled. Windows also links the CRT
statically.

```sh
python3 scripts/build.py
python3 scripts/build.py --offline
```

On Windows the script locates and initializes Visual Studio C++ and Windows SDK
tools. It supplies the cl.exe/link.exe directory to the FFmpeg recipe's MSYS2
build, so a Developer Command Prompt is not required. The first build accesses
Conan remotes; `--offline` uses only cached dependencies.

The Windows executable is `dist/mirctl.exe`. It does not require bundled FFmpeg,
SDL, VCRUNTIME or UCRT DLLs; Windows system DLLs remain OS dependencies. Public
binary releases must include the corresponding dependency source, license and
rebuild materials described in [building and packaging](../../docs/building.md).

For manual Windows builds, run the following in an **x64 Native Tools Command
Prompt for VS 2022**:

```sh
conan install . --build=missing -s:h build_type=Release -s:h compiler.runtime=static -c:h tools.cmake.cmaketoolchain:generator=Ninja -o "ffmpeg/*:enable_hardware_accelerators=h264_d3d11va,h264_d3d11va2,h264_dxva2,hevc_d3d11va,hevc_d3d11va2,hevc_dxva2,av1_d3d11va,av1_d3d11va2,av1_dxva2"
cmake -S . -B build/app/Release -G "NMake Makefiles" -DCMAKE_BUILD_TYPE=Release -DCMAKE_TOOLCHAIN_FILE=build/Release/generators/conan_toolchain.cmake
cmake --build build/app/Release
```

## Runtime behavior

The Godot demo listens on `127.0.0.1:20831`. Start the game before the client:

```sh
mirctl
mirctl 192.168.1.20:20831
mirctl --no-control 127.0.0.1:20831
mirctl --codec=av1 127.0.0.1:20831   # Select AV1 on the Godot host too.
mirctl --codec=hevc 127.0.0.1:20831  # Select HEVC on the Godot host too.
mirctl "[::1]:20831"
```

The client supports YUV420P, hardware-decoded NV12 and 10-bit P010 frames,
dynamic resolution/pixel-format changes, common keyboard keys, arrow/function
keys, the numeric keypad, mouse motion/buttons, double clicks and the wheel.
Users can resize the window after its initial automatic sizing. Input coordinates
are mapped through the letterboxed video rectangle; black bars do not produce
game input.

## Protocol limits

- Video is an elementary stream without packet-length prefixes. H.264/HEVC
  require parameter sets and a keyframe; AV1 requires temporal delimiters,
  sequence headers and a keyframe. TCP chunks need not match frame boundaries.
- There are no timestamps, handshake or codec negotiation. Match `--codec` to
  the Godot host setting, or `GAME_STREAM_CODEC` when using the native API's
  environment configuration. Negotiated codecs, audio or browser clients would
  need a separate session/media description layer.
- GSI1 input shares the TCP connection; integers and floating-point values use
  little-endian encoding.
- TCP provides reliability with head-of-line blocking. The display mailbox
  prevents decoded frames from accumulating but cannot remove backlog already
  inside TCP or the encoder.
- Small control messages are written synchronously on the SDL event thread.
  A stalled network can briefly block that thread. Stronger isolation would
  require a bounded control queue and a dedicated writer.
- The protocol has no authentication or encryption. Use a trusted tunnel or
  add authenticated transport before exposing a listener outside loopback;
  connected clients can view video and inject input when control is enabled.
- Only the server's TCP listener mode is supported. UDP, WebSocket and audio
  are not implemented in this client.

## Formatting

```sh
python3 scripts/format.py
python3 scripts/format.py --check
```
