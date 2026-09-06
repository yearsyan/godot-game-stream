// SPDX-License-Identifier: LGPL-2.1-or-later
// Copyright (C) 2026 yearsyan and contributors

use std::collections::VecDeque;
use std::io::Read;
use std::net::TcpStream;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::Duration;

use godot::builtin::Vector2;
use godot::classes::{
    Input, InputEvent, InputEventKey, InputEventMouseButton, InputEventMouseMotion,
};
use godot::global::{Key, MouseButton};
use godot::obj::NewGd;
use godot::prelude::*;

use crate::shared::SharedState;

/// Reverse input channel "GSI1": streaming clients send framed input events
/// upstream over the same connection that carries the H.264 stream down. The
/// wire format (all integers little-endian):
///
/// ```text
/// u32 magic = 0x47534931 ("GSI1")
/// u16 payload_len
/// payload:
///   type=1 key:           u8 pressed, u32 keycode, u32 unicode          (10 B)
///   type=2 mouse button:  u8 button, u8 pressed, u8 double, u8 pad,
///                         f32 x, f32 y                                  (13 B)
///   type=3 mouse motion:  f32 x, f32 y, f32 dx, f32 dy                  (17 B)
/// ```
///
/// `keycode` is a Godot `Key` ordinal (client-side mapping), `button` a Godot
/// `MouseButton` ordinal (wheel buttons 4..=7 arrive as button presses), and
/// coordinates are in stream-output pixels scaled to the viewport on inject.
pub const INPUT_PROTOCOL_MAGIC: u32 = 0x4753_4931;

const HEADER_LEN: usize = 6;
const MAX_PAYLOAD_LEN: usize = 64;
const MAX_STREAM_BUFFER: usize = 8 * 1024;
const QUEUE_CAPACITY: usize = 256;
const INPUT_READ_POLL: Duration = Duration::from_millis(250);

const EVENT_KEY: u8 = 1;
const EVENT_MOUSE_BUTTON: u8 = 2;
const EVENT_MOUSE_MOTION: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InputCommand {
    Key {
        keycode: u32,
        unicode: u32,
        pressed: bool,
    },
    MouseButton {
        button: u8,
        pressed: bool,
        double_click: bool,
        x: f32,
        y: f32,
    },
    MouseMotion {
        x: f32,
        y: f32,
        dx: f32,
        dy: f32,
    },
}

/// Parses one complete framed message (UDP: one datagram; TCP: one frame
/// extracted by `MessageReader`). Extra trailing bytes are rejected.
pub fn parse_datagram(bytes: &[u8]) -> Option<InputCommand> {
    let magic = u32::from_le_bytes(bytes.get(0..4)?.try_into().ok()?);
    if magic != INPUT_PROTOCOL_MAGIC {
        return None;
    }
    let payload_len = u16::from_le_bytes(bytes.get(4..6)?.try_into().ok()?) as usize;
    if bytes.len() != HEADER_LEN + payload_len || payload_len > MAX_PAYLOAD_LEN {
        return None;
    }
    let payload = bytes.get(HEADER_LEN..)?;
    parse_payload(payload)
}

fn parse_payload(payload: &[u8]) -> Option<InputCommand> {
    match *payload.first()? {
        EVENT_KEY if payload.len() == 10 => Some(InputCommand::Key {
            pressed: payload[1] != 0,
            keycode: u32::from_le_bytes(payload[2..6].try_into().ok()?),
            unicode: u32::from_le_bytes(payload[6..10].try_into().ok()?),
        }),
        EVENT_MOUSE_BUTTON if payload.len() == 13 => {
            let x = f32::from_le_bytes(payload[5..9].try_into().ok()?);
            let y = f32::from_le_bytes(payload[9..13].try_into().ok()?);
            if !(1..=9).contains(&payload[1]) || !x.is_finite() || !y.is_finite() {
                return None;
            }
            Some(InputCommand::MouseButton {
                button: payload[1],
                pressed: payload[2] != 0,
                double_click: payload[3] != 0,
                x,
                y,
            })
        }
        EVENT_MOUSE_MOTION if payload.len() == 17 => {
            let x = f32::from_le_bytes(payload[1..5].try_into().ok()?);
            let y = f32::from_le_bytes(payload[5..9].try_into().ok()?);
            let dx = f32::from_le_bytes(payload[9..13].try_into().ok()?);
            let dy = f32::from_le_bytes(payload[13..17].try_into().ok()?);
            (x.is_finite() && y.is_finite() && dx.is_finite() && dy.is_finite())
                .then_some(InputCommand::MouseMotion { x, y, dx, dy })
        }
        _ => None,
    }
}

/// Encodes a command into a framed message (used by tests; also the reference
/// for client implementations).
#[cfg_attr(not(test), allow(dead_code))]
pub fn encode_message(command: InputCommand) -> Vec<u8> {
    let payload: Vec<u8> = match command {
        InputCommand::Key {
            keycode,
            unicode,
            pressed,
        } => {
            let mut payload = vec![EVENT_KEY, u8::from(pressed)];
            payload.extend_from_slice(&keycode.to_le_bytes());
            payload.extend_from_slice(&unicode.to_le_bytes());
            payload
        }
        InputCommand::MouseButton {
            button,
            pressed,
            double_click,
            x,
            y,
        } => {
            let mut payload = vec![
                EVENT_MOUSE_BUTTON,
                button,
                u8::from(pressed),
                u8::from(double_click),
                0,
            ];
            payload.extend_from_slice(&x.to_le_bytes());
            payload.extend_from_slice(&y.to_le_bytes());
            payload
        }
        InputCommand::MouseMotion { x, y, dx, dy } => {
            let mut payload = vec![EVENT_MOUSE_MOTION];
            payload.extend_from_slice(&x.to_le_bytes());
            payload.extend_from_slice(&y.to_le_bytes());
            payload.extend_from_slice(&dx.to_le_bytes());
            payload.extend_from_slice(&dy.to_le_bytes());
            payload
        }
    };
    let mut bytes = Vec::with_capacity(HEADER_LEN + payload.len());
    bytes.extend_from_slice(&INPUT_PROTOCOL_MAGIC.to_le_bytes());
    bytes.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    bytes.extend_from_slice(&payload);
    bytes
}

/// Incremental frame extractor for the TCP upstream. Video-only clients such
/// as ffplay never send anything; garbage on the socket resynchronizes one
/// byte at a time until the magic reappears.
pub struct MessageReader {
    buffer: Vec<u8>,
}

impl MessageReader {
    pub fn new() -> Self {
        Self {
            buffer: Vec::with_capacity(256),
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Vec<InputCommand> {
        self.buffer.extend_from_slice(bytes);
        let mut commands = Vec::new();
        let mut consumed = 0;
        loop {
            let rest = &self.buffer[consumed..];
            if rest.len() < HEADER_LEN {
                break;
            }
            let magic = u32::from_le_bytes(rest[0..4].try_into().expect("four bytes"));
            if magic != INPUT_PROTOCOL_MAGIC {
                consumed += 1;
                continue;
            }
            let payload_len =
                u16::from_le_bytes(rest[4..6].try_into().expect("two bytes")) as usize;
            if payload_len > MAX_PAYLOAD_LEN {
                consumed += 4;
                continue;
            }
            let frame_len = HEADER_LEN + payload_len;
            if rest.len() < frame_len {
                break;
            }
            if let Some(command) = parse_payload(&rest[HEADER_LEN..frame_len]) {
                commands.push(command);
            }
            consumed += frame_len;
        }
        self.buffer.drain(..consumed);
        if self.buffer.len() > MAX_STREAM_BUFFER {
            self.buffer.clear();
        }
        commands
    }
}

/// Bounded hand-off between the network reader threads and the main-thread
/// `frame_post_draw` drain. Overflow drops the oldest event so a flood of
/// motion events can never starve keys or clicks.
pub struct InputQueue {
    events: Mutex<VecDeque<InputCommand>>,
}

impl InputQueue {
    pub fn new() -> Self {
        Self {
            events: Mutex::new(VecDeque::with_capacity(QUEUE_CAPACITY)),
        }
    }

    pub fn push(&self, command: InputCommand) {
        let mut events = self.events.lock().expect("input queue mutex");
        if events.len() >= QUEUE_CAPACITY {
            events.pop_front();
        }
        events.push_back(command);
    }

    pub fn take_all(&self) -> Vec<InputCommand> {
        let mut events = self.events.lock().expect("input queue mutex");
        events.drain(..).collect()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.events.lock().expect("input queue mutex").len()
    }
}

/// Per-client TCP upstream reader. Reading (even when nothing is parsed) also
/// keeps a video-only client's upstream from back-pressuring the server's
/// video writes.
pub fn run_tcp_input_reader(shared: std::sync::Arc<SharedState>, mut stream: TcpStream) {
    if stream.set_read_timeout(Some(INPUT_READ_POLL)).is_err() {
        return;
    }
    let mut reader = MessageReader::new();
    let mut buffer = [0u8; 2048];
    loop {
        if !shared.running.load(Ordering::SeqCst) {
            return;
        }
        match stream.read(&mut buffer) {
            Ok(0) => return,
            Ok(len) => accept_commands(&shared, reader.feed(&buffer[..len])),
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => return,
        }
    }
}

/// Drains pending UDP input datagrams from the (non-blocking) video send
/// socket. Only datagrams from the configured video destination are accepted.
pub fn drain_udp_input(
    shared: &SharedState,
    socket: &std::net::UdpSocket,
    expected: std::net::SocketAddr,
) {
    let mut buffer = [0u8; HEADER_LEN + MAX_PAYLOAD_LEN];
    loop {
        match socket.recv_from(&mut buffer) {
            Ok((len, source)) => {
                if source != expected {
                    continue;
                }
                if let Some(command) = parse_datagram(&buffer[..len]) {
                    accept_commands(shared, [command]);
                }
            }
            Err(_) => return,
        }
    }
}

fn accept_commands(shared: &SharedState, commands: impl IntoIterator<Item = InputCommand>) {
    let mut received = 0u64;
    for command in commands {
        shared.input_queue.push(command);
        received += 1;
    }
    shared
        .input_events_received
        .fetch_add(received, Ordering::Relaxed);
}

/// Main-thread injection: drains the queue and replays the events through
/// `Input.parse_input_event`, so the full Godot input pipeline (actions, UI,
/// unhandled input) sees them exactly like local input.
pub fn inject_pending(shared: &SharedState) {
    let commands = shared.input_queue.take_all();
    if commands.is_empty() || !shared.input_enabled {
        return;
    }

    let scale = viewport_scale(shared);
    let mut input = Input::singleton();
    let mut injected = 0u64;
    let mut dropped = 0u64;
    for command in commands {
        let event: Gd<InputEvent> = match command {
            InputCommand::Key {
                keycode,
                unicode,
                pressed,
            } => {
                if keycode == 0 || keycode > i32::MAX as u32 {
                    dropped += 1;
                    continue;
                }
                let key = Key::from_ord(keycode as i32);
                let mut event = InputEventKey::new_gd();
                event.set_keycode(key);
                event.set_physical_keycode(key);
                event.set_unicode(unicode);
                event.set_pressed(pressed);
                event.set_echo(false);
                event.upcast()
            }
            InputCommand::MouseButton {
                button,
                pressed,
                double_click,
                x,
                y,
            } => {
                let Some((scale_x, scale_y)) = scale else {
                    dropped += 1;
                    continue;
                };
                let button_index = MouseButton::from_ord(i32::from(button));
                let mut event = InputEventMouseButton::new_gd();
                event.set_button_index(button_index);
                event.set_pressed(pressed);
                event.set_double_click(double_click);
                event.set_position(Vector2::new(x * scale_x, y * scale_y));
                if (MouseButton::WHEEL_UP.ord()..=MouseButton::WHEEL_RIGHT.ord())
                    .contains(&button_index.ord())
                {
                    event.set_factor(1.0);
                }
                event.upcast()
            }
            InputCommand::MouseMotion { x, y, dx, dy } => {
                let Some((scale_x, scale_y)) = scale else {
                    dropped += 1;
                    continue;
                };
                let mut event = InputEventMouseMotion::new_gd();
                event.set_position(Vector2::new(x * scale_x, y * scale_y));
                event.set_relative(Vector2::new(dx * scale_x, dy * scale_y));
                event.set_velocity(Vector2::ZERO);
                event.upcast()
            }
        };
        input.parse_input_event(&event);
        injected += 1;
    }
    shared
        .input_events_injected
        .fetch_add(injected, Ordering::Relaxed);
    shared
        .input_events_dropped
        .fetch_add(dropped, Ordering::Relaxed);
}

/// Stream-output pixels -> root viewport pixels; unknown until the first
/// captured frame reports both sizes.
fn viewport_scale(shared: &SharedState) -> Option<(f32, f32)> {
    let source_width = shared.last_width.load(Ordering::Relaxed);
    let source_height = shared.last_height.load(Ordering::Relaxed);
    let output_width = shared.output_width.load(Ordering::Relaxed);
    let output_height = shared.output_height.load(Ordering::Relaxed);
    if source_width == 0 || source_height == 0 || output_width == 0 || output_height == 0 {
        return None;
    }
    Some((
        source_width as f32 / output_width as f32,
        source_height as f32 / output_height as f32,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_client_fixtures_match_parser_and_serializer() {
        let fixtures: &[(&[u8], InputCommand)] = &[
            (
                include_bytes!("../../../protocol/fixtures/key-a.bin"),
                InputCommand::Key {
                    keycode: 65,
                    unicode: 97,
                    pressed: true,
                },
            ),
            (
                include_bytes!("../../../protocol/fixtures/mouse-left.bin"),
                InputCommand::MouseButton {
                    button: 1,
                    pressed: true,
                    double_click: false,
                    x: 100.5,
                    y: 200.25,
                },
            ),
            (
                include_bytes!("../../../protocol/fixtures/mouse-motion.bin"),
                InputCommand::MouseMotion {
                    x: 100.5,
                    y: 200.25,
                    dx: -2.0,
                    dy: 3.5,
                },
            ),
        ];
        for &(bytes, command) in fixtures {
            assert_eq!(parse_datagram(bytes), Some(command));
            assert_eq!(encode_message(command), bytes);
        }
    }

    #[test]
    fn encode_parse_round_trips_every_event_type() {
        let commands = [
            InputCommand::Key {
                keycode: 4_194_305,
                unicode: 0,
                pressed: true,
            },
            InputCommand::Key {
                keycode: 65,
                unicode: 97,
                pressed: false,
            },
            InputCommand::MouseButton {
                button: 1,
                pressed: true,
                double_click: false,
                x: 640.5,
                y: 360.25,
            },
            InputCommand::MouseMotion {
                x: 1.0,
                y: 2.0,
                dx: -0.5,
                dy: 3.5,
            },
        ];
        for command in commands {
            let message = encode_message(command);
            assert_eq!(parse_datagram(&message), Some(command));
        }
    }

    #[test]
    fn key_message_layout_is_fixed() {
        let message = encode_message(InputCommand::Key {
            keycode: 4_194_309,
            unicode: 13,
            pressed: true,
        });
        assert_eq!(message.len(), 16);
        assert_eq!(&message[..4], &INPUT_PROTOCOL_MAGIC.to_le_bytes());
        assert_eq!(u16::from_le_bytes([message[4], message[5]]), 10);
        assert_eq!(message[6], EVENT_KEY);
        assert_eq!(message[7], 1);
        assert_eq!(
            u32::from_le_bytes(message[8..12].try_into().unwrap()),
            4_194_309
        );
        assert_eq!(u32::from_le_bytes(message[12..16].try_into().unwrap()), 13);
    }

    #[test]
    fn datagram_parser_rejects_bad_input() {
        let valid = encode_message(InputCommand::MouseMotion {
            x: 1.0,
            y: 1.0,
            dx: 0.0,
            dy: 0.0,
        });
        assert_eq!(parse_datagram(&valid[..valid.len() - 1]), None);
        let mut trailing = valid.clone();
        trailing.push(0);
        assert_eq!(parse_datagram(&trailing), None);

        let mut bad_magic = valid;
        bad_magic[0] ^= 0xFF;
        assert_eq!(parse_datagram(&bad_magic), None);

        let nan_coords = encode_message(InputCommand::MouseMotion {
            x: f32::NAN,
            y: 0.0,
            dx: 0.0,
            dy: 0.0,
        });
        assert_eq!(parse_datagram(&nan_coords), None);

        let bad_button = encode_message(InputCommand::MouseButton {
            button: 42,
            pressed: true,
            double_click: false,
            x: 1.0,
            y: 1.0,
        });
        assert_eq!(parse_datagram(&bad_button), None);
    }

    #[test]
    fn reader_assembles_frames_across_partial_reads_and_resyncs_on_garbage() {
        let first = encode_message(InputCommand::Key {
            keycode: 32,
            unicode: 32,
            pressed: true,
        });
        let second = encode_message(InputCommand::MouseButton {
            button: 3,
            pressed: false,
            double_click: true,
            x: 10.0,
            y: 20.0,
        });
        let mut stream = b"\xDE\xAD\xBE\xEFgarbage".to_vec();
        stream.extend_from_slice(&first);
        stream.extend_from_slice(&second);

        let mut reader = MessageReader::new();
        assert!(reader.feed(&stream[..3]).is_empty());
        let commands = reader.feed(&stream[3..]);
        assert_eq!(
            commands,
            vec![
                InputCommand::Key {
                    keycode: 32,
                    unicode: 32,
                    pressed: true
                },
                InputCommand::MouseButton {
                    button: 3,
                    pressed: false,
                    double_click: true,
                    x: 10.0,
                    y: 20.0
                },
            ]
        );
    }

    #[test]
    fn queue_drops_oldest_when_full() {
        let queue = InputQueue::new();
        for index in 0..QUEUE_CAPACITY + 10 {
            queue.push(InputCommand::Key {
                keycode: index as u32 + 1,
                unicode: 0,
                pressed: true,
            });
        }
        assert_eq!(queue.len(), QUEUE_CAPACITY);
        let drained = queue.take_all();
        assert_eq!(drained.len(), QUEUE_CAPACITY);
        assert_eq!(
            drained.first(),
            Some(&InputCommand::Key {
                keycode: 11,
                unicode: 0,
                pressed: true
            })
        );
        assert!(queue.take_all().is_empty());
    }
}
