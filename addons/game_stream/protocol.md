# Game Stream wire protocol (v1)

This specifies the existing protocol. The standalone extraction preserves its
bytes. TCP is full duplex: server-to-client video and client-to-server input.
The conventional default endpoint is `127.0.0.1:20831`.

## Video

H.264/HEVC use raw Annex-B elementary streams with three/four-byte start codes.
AV1 uses low-overhead OBU with temporal-delimiter boundaries. There is no custom
length prefix, session handshake, authentication, encryption, audio or codec
negotiation. TCP reads may split any unit at any byte.

A new client receives parameter sets and a decoder-reset keyframe. H.264 requires
SPS/PPS; HEVC uses VPS/SPS/PPS; AV1 carries the sequence header in its keyframe
stream. A dropped encoded frame must be followed by recovery at a new keyframe
before dependent frames are delivered. Resolution changes retain the chosen codec.

Default: H.264 on both ends. Explicit HEVC/AV1 must be selected on both ends.
The native UDP sender splits raw bytes into datagrams; mirctl does not implement
that transport. It is not RTP and does not add reliability or reassembly metadata.

## GSI1 input

All integers and IEEE-754 binary32 floats are **little endian**.

| Offset | Type | Meaning |
| --- | --- | --- |
| 0 | u32 | Numeric magic `0x47534931` |
| 4 | u16 | Payload byte length, excluding this 6-byte header |
| 6 | bytes | Payload |

The actual four magic bytes are **`31 49 53 47` (`1ISG`)**. Do not replace them
with ASCII `GSI1`: that would break existing clients.

| Event | Payload layout | Length |
| --- | --- | --- |
| Key | `u8(1), u8 pressed, u32 Godot keycode, u32 unicode` | 10 |
| Mouse button | `u8(2), u8 button, u8 pressed, u8 double_click, u8 reserved(0), f32 x, f32 y` | 13 |
| Mouse motion | `u8(3), f32 x, f32 y, f32 dx, f32 dy` | 17 |

Coordinates are video-output pixels before conversion to root-viewport coordinates.
Button values use Godot MouseButton: 1 left, 2 right, 3 middle, 4–7 wheel,
8/9 auxiliary. Wheel events are button press/release pairs. Keys use Godot Key
ordinals; special keys use the `1 << 22` flag. Floats must be finite.

The server limits payloads to 64 bytes and validates event lengths. Its TCP parser
assembles partial messages and resynchronizes after invalid magic. Input queues
are bounded. This protocol currently has no text/IME or gamepad message type.

`protocol/fixtures/` contains byte-exact key, mouse-button and mouse-motion messages.
Rust parser/serializer tests and the mirctl C test consume the same files.
