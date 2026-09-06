// SPDX-License-Identifier: LGPL-2.1-or-later
// Copyright (C) 2026 yearsyan and contributors

use std::io::{ErrorKind, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::annexb;
use crate::shared::{EncodedFrame, FrameTimingSample, SharedState, StreamCodec};

const MAILBOX_WAIT: Duration = Duration::from_millis(20);
// Large key frames at high resolutions can legitimately stall a client's TCP
// recv window for tens of milliseconds while it decodes; 50ms kicked healthy
// clients during scene changes. 250ms still removes truly stalled peers
// within a few frames.
const CLIENT_WRITE_TIMEOUT: Duration = Duration::from_millis(250);
const UDP_PAYLOAD_SIZE: usize = 1_200;
const UDP_ERROR_LOG_INTERVAL: Duration = Duration::from_secs(1);

pub enum Transport {
    Tcp(TcpListener),
    Udp {
        socket: UdpSocket,
        destination: SocketAddr,
    },
}

struct Client {
    stream: TcpStream,
    waiting_for_keyframe: bool,
}

#[derive(Default)]
struct ParameterSets {
    vps: Option<Vec<u8>>,
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
}

impl ParameterSets {
    fn headers(&self, codec: StreamCodec) -> [Option<&[u8]>; 3] {
        match codec {
            StreamCodec::Hevc => [
                self.vps.as_deref(),
                self.sps.as_deref(),
                self.pps.as_deref(),
            ],
            StreamCodec::H264 => [None, self.sps.as_deref(), self.pps.as_deref()],
            StreamCodec::Auto | StreamCodec::Av1 => [None, None, None],
        }
    }
}

pub fn run_net_thread(shared: Arc<SharedState>, transport: Transport) {
    match transport {
        Transport::Tcp(listener) => run_tcp_thread(shared, listener),
        Transport::Udp {
            socket,
            destination,
        } => run_udp_thread(shared, socket, destination),
    }
}

fn run_tcp_thread(shared: Arc<SharedState>, listener: TcpListener) {
    let _ = listener.set_nonblocking(true);
    let mut clients: Vec<Client> = Vec::new();
    let mut parameter_sets = ParameterSets::default();
    let mut last_keyframe: Option<Vec<Vec<u8>>> = None;
    let mut cached_codec = StreamCodec::Auto;
    // Sequence of the last frame taken from the mailbox. The mailbox is
    // capacity-1 and replaces undelivered frames, so a gap means clients
    // never received a frame that later inter frames may reference.
    let mut last_sequence: Option<u64> = None;

    while shared.running.load(Ordering::SeqCst) {
        let codec = *shared.active_codec.lock().expect("active codec mutex");
        if codec != cached_codec {
            parameter_sets = ParameterSets::default();
            last_keyframe = None;
            cached_codec = codec;
        }
        accept_pending(
            &shared,
            &listener,
            &mut clients,
            &parameter_sets,
            last_keyframe.as_deref(),
            codec,
        );
        let Some(frame) = shared
            .encoded_mailbox
            .wait_take(&shared.running, MAILBOX_WAIT)
        else {
            continue;
        };
        let network_started_at = Instant::now();

        if frame.codec != cached_codec {
            parameter_sets = ParameterSets::default();
            last_keyframe = None;
            cached_codec = frame.codec;
            clients
                .iter_mut()
                .for_each(|client| client.waiting_for_keyframe = true);
        }
        // An inter frame behind a dropped frame would reference references no
        // client has; feeding it to a raw-stream decoder is fatal there. Hold
        // all traffic until the encoder's forced key frame arrives. Key
        // frames are self-contained and always safe to deliver.
        if !frame.keyframe && last_sequence.is_some_and(|last| frame.sequence > last + 1) {
            clients
                .iter_mut()
                .for_each(|client| client.waiting_for_keyframe = true);
            last_sequence = Some(frame.sequence);
            godot::prelude::godot_warn!(
                "game_stream: dropped frame(s) before sequence {}; holding clients until key frame",
                frame.sequence
            );
            continue;
        }
        last_sequence = Some(frame.sequence);
        for packet in &frame.packets {
            update_parameter_sets(packet, frame.codec, &mut parameter_sets);
        }

        let bytes = frame.packets.iter().map(Vec::len).sum::<usize>() as u64;
        let is_keyframe = frame.keyframe;
        if is_keyframe {
            last_keyframe = Some(frame.packets.clone());
        }
        let clients_before = clients.len();
        let mut sent_to_any = false;
        let write_started = Instant::now();
        clients.retain_mut(|client| {
            if client.waiting_for_keyframe && !is_keyframe {
                return true;
            }
            match write_access_unit(&mut client.stream, &frame) {
                Ok(()) => {
                    client.waiting_for_keyframe = false;
                    sent_to_any = true;
                    true
                }
                Err(_) => false,
            }
        });
        let completed_at = Instant::now();
        let socket_write_us = completed_at
            .saturating_duration_since(write_started)
            .as_micros() as u64;

        if sent_to_any {
            record_delivery(
                &shared,
                &frame,
                network_started_at,
                write_started,
                completed_at,
                socket_write_us,
                bytes,
            );
        }
        if clients.len() != clients_before {
            godot::prelude::godot_warn!(
                "game_stream: removed {} slow/disconnected client(s) while sending frame {}",
                clients_before - clients.len(),
                frame.sequence
            );
            shared.force_keyframe.store(true, Ordering::SeqCst);
        }
        shared
            .client_count
            .store(clients.len() as u32, Ordering::SeqCst);
    }

    shared.client_count.store(0, Ordering::SeqCst);
}

fn run_udp_thread(shared: Arc<SharedState>, socket: UdpSocket, destination: SocketAddr) {
    shared.client_count.store(1, Ordering::SeqCst);
    shared.force_keyframe.store(true, Ordering::SeqCst);
    let mut parameter_sets = ParameterSets::default();
    let mut cached_codec = StreamCodec::Auto;
    let mut last_error_log = Instant::now()
        .checked_sub(UDP_ERROR_LOG_INTERVAL)
        .unwrap_or_else(Instant::now);

    godot::prelude::godot_print!(
        "game_stream: UDP transport active to {destination} ({UDP_PAYLOAD_SIZE}-byte payloads, no retransmit/backlog)"
    );

    while shared.running.load(Ordering::SeqCst) {
        crate::input::drain_udp_input(&shared, &socket, destination);
        let Some(frame) = shared
            .encoded_mailbox
            .wait_take(&shared.running, MAILBOX_WAIT)
        else {
            continue;
        };
        let network_started_at = Instant::now();
        if frame.codec != cached_codec {
            parameter_sets = ParameterSets::default();
            cached_codec = frame.codec;
        }
        for packet in &frame.packets {
            update_parameter_sets(packet, frame.codec, &mut parameter_sets);
        }
        let is_keyframe = frame.keyframe;
        let write_started = Instant::now();
        let headers = if is_keyframe {
            parameter_sets.headers(frame.codec)
        } else {
            [None, None, None]
        };
        let result = send_udp_access_unit(&socket, destination, &frame, headers);
        let completed_at = Instant::now();
        let socket_write_us = completed_at
            .saturating_duration_since(write_started)
            .as_micros() as u64;

        match result {
            Ok(bytes) => record_delivery(
                &shared,
                &frame,
                network_started_at,
                write_started,
                completed_at,
                socket_write_us,
                bytes,
            ),
            Err(error) => {
                shared.dropped_after_encode.fetch_add(1, Ordering::Relaxed);
                shared.force_keyframe.store(true, Ordering::SeqCst);
                if last_error_log.elapsed() >= UDP_ERROR_LOG_INTERVAL {
                    godot::prelude::godot_warn!(
                        "game_stream: UDP send to {destination} failed; dropping frame {}: {error}",
                        frame.sequence
                    );
                    last_error_log = Instant::now();
                }
            }
        }
    }

    shared.client_count.store(0, Ordering::SeqCst);
}

fn send_udp_access_unit(
    socket: &UdpSocket,
    destination: SocketAddr,
    frame: &EncodedFrame,
    headers: [Option<&[u8]>; 3],
) -> std::io::Result<u64> {
    let mut bytes = 0;
    for header in headers.into_iter().flatten() {
        bytes += send_udp_bytes(socket, destination, header)?;
    }
    for packet in &frame.packets {
        bytes += send_udp_bytes(socket, destination, packet)?;
    }
    Ok(bytes)
}

fn send_udp_bytes(
    socket: &UdpSocket,
    destination: SocketAddr,
    bytes: &[u8],
) -> std::io::Result<u64> {
    let mut sent_total = 0;
    for chunk in bytes.chunks(UDP_PAYLOAD_SIZE) {
        let sent = socket.send_to(chunk, destination)?;
        if sent != chunk.len() {
            return Err(std::io::Error::new(
                ErrorKind::WriteZero,
                format!(
                    "UDP datagram truncated: sent {sent} of {} bytes",
                    chunk.len()
                ),
            ));
        }
        sent_total += sent as u64;
    }
    Ok(sent_total)
}

fn record_delivery(
    shared: &SharedState,
    frame: &EncodedFrame,
    network_started_at: Instant,
    write_started: Instant,
    completed_at: Instant,
    socket_write_us: u64,
    bytes: u64,
) {
    shared.bytes_sent.fetch_add(bytes, Ordering::Relaxed);
    shared.network_queue_us.store(
        network_started_at
            .saturating_duration_since(frame.encoded_at)
            .as_micros() as u64,
        Ordering::Relaxed,
    );
    shared.packet_prep_us.store(
        write_started
            .saturating_duration_since(network_started_at)
            .as_micros() as u64,
        Ordering::Relaxed,
    );
    shared
        .socket_write_us
        .store(socket_write_us, Ordering::Relaxed);
    shared.capture_to_socket_us.store(
        completed_at
            .saturating_duration_since(frame.timeline.post_draw_at)
            .as_micros() as u64,
        Ordering::Relaxed,
    );
    shared
        .last_access_unit_bytes
        .store(bytes, Ordering::Relaxed);
    shared.record_timing(FrameTimingSample {
        captured_at: frame.timeline.post_draw_at,
        completed_at,
        post_to_render_us: frame
            .timeline
            .render_thread_at
            .saturating_duration_since(frame.timeline.post_draw_at)
            .as_micros() as u64,
        render_setup_us: frame
            .timeline
            .readback_submitted_at
            .saturating_duration_since(frame.timeline.render_thread_at)
            .as_micros() as u64,
        readback_wait_us: frame
            .timeline
            .callback_at
            .saturating_duration_since(frame.timeline.readback_submitted_at)
            .as_micros() as u64,
        callback_copy_us: frame
            .timeline
            .ready_for_encode_at
            .saturating_duration_since(frame.timeline.callback_at)
            .as_micros() as u64,
        encode_queue_us: frame
            .encode_started_at
            .saturating_duration_since(frame.timeline.ready_for_encode_at)
            .as_micros() as u64,
        scale_us: frame.scale_us,
        encode_us: frame.encode_us,
        encoder_stage_us: frame
            .encoded_at
            .saturating_duration_since(frame.encode_started_at)
            .as_micros() as u64,
        capture_to_encoded_us: frame
            .encoded_at
            .saturating_duration_since(frame.timeline.post_draw_at)
            .as_micros() as u64,
        network_queue_us: network_started_at
            .saturating_duration_since(frame.encoded_at)
            .as_micros() as u64,
        packet_prep_us: write_started
            .saturating_duration_since(network_started_at)
            .as_micros() as u64,
        socket_write_us,
        total_us: completed_at
            .saturating_duration_since(frame.timeline.post_draw_at)
            .as_micros() as u64,
        frame_bytes: bytes,
    });
}

fn accept_pending(
    shared: &Arc<SharedState>,
    listener: &TcpListener,
    clients: &mut Vec<Client>,
    parameter_sets: &ParameterSets,
    last_keyframe: Option<&[Vec<u8>]>,
    codec: StreamCodec,
) {
    loop {
        match listener.accept() {
            Ok((stream, addr)) => {
                let setup = prepare_client(&stream)
                    .and_then(|_| send_headers(&stream, parameter_sets.headers(codec)))
                    .and_then(|_| send_cached_keyframe(&stream, last_keyframe));
                match setup {
                    Ok(()) => {
                        godot::prelude::godot_print!(
                            "game_stream: client connected {addr}{}",
                            if last_keyframe.is_some() {
                                ", sent cached key frame"
                            } else {
                                ", waiting for key frame"
                            }
                        );
                        if let Ok(upstream) = stream.try_clone() {
                            let reader_shared = Arc::clone(shared);
                            if let Err(error) = std::thread::Builder::new()
                                .name("game_stream_input".into())
                                .spawn(move || {
                                    crate::input::run_tcp_input_reader(reader_shared, upstream)
                                })
                            {
                                godot::prelude::godot_warn!(
                                    "game_stream: input reader spawn failed: {error}"
                                );
                            }
                        }
                        // Cached key frame lets the decoder paint immediately.
                        // Keep waiting_for_keyframe so later inter frames from a
                        // newer GOP are not sent until the forced live key
                        // frame arrives.
                        clients.push(Client {
                            stream,
                            waiting_for_keyframe: true,
                        });
                        shared
                            .client_count
                            .store(clients.len() as u32, Ordering::SeqCst);
                        shared.force_keyframe.store(true, Ordering::SeqCst);
                    }
                    Err(error) => {
                        godot::prelude::godot_warn!(
                            "game_stream: client {addr} setup failed: {error}"
                        );
                    }
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => break,
            Err(error) => {
                godot::prelude::godot_warn!("game_stream: accept failed: {error}");
                break;
            }
        }
    }
}

fn prepare_client(stream: &TcpStream) -> std::io::Result<()> {
    stream.set_nodelay(true)?;
    stream.set_nonblocking(false)?;
    stream.set_write_timeout(Some(CLIENT_WRITE_TIMEOUT))?;
    Ok(())
}

fn send_headers(stream: &TcpStream, headers: [Option<&[u8]>; 3]) -> std::io::Result<()> {
    let mut stream = stream;
    for header in headers.into_iter().flatten() {
        stream.write_all(header)?;
    }
    Ok(())
}

fn send_cached_keyframe(
    stream: &TcpStream,
    last_keyframe: Option<&[Vec<u8>]>,
) -> std::io::Result<()> {
    let Some(packets) = last_keyframe else {
        return Ok(());
    };
    let mut stream = stream;
    for packet in packets {
        stream.write_all(packet)?;
    }
    Ok(())
}

fn write_access_unit(stream: &mut TcpStream, frame: &EncodedFrame) -> std::io::Result<()> {
    for packet in &frame.packets {
        stream.write_all(packet)?;
    }
    Ok(())
}

fn update_parameter_sets(packet: &[u8], codec: StreamCodec, sets: &mut ParameterSets) {
    match codec {
        StreamCodec::H264 => {
            for nal in annexb::iter_nals(packet) {
                match annexb::h264_nal_type(nal) {
                    Some(annexb::H264_SPS) => sets.sps = Some(nal.to_vec()),
                    Some(annexb::H264_PPS) => sets.pps = Some(nal.to_vec()),
                    _ => {}
                }
            }
        }
        StreamCodec::Hevc => {
            for nal in annexb::iter_nals(packet) {
                match annexb::hevc_nal_type(nal) {
                    Some(annexb::HEVC_VPS) => sets.vps = Some(nal.to_vec()),
                    Some(annexb::HEVC_SPS) => sets.sps = Some(nal.to_vec()),
                    Some(annexb::HEVC_PPS) => sets.pps = Some(nal.to_vec()),
                    _ => {}
                }
            }
        }
        StreamCodec::Auto | StreamCodec::Av1 => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn udp_sender_splits_payload_without_changing_bytes() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let destination = receiver.local_addr().unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let payload = (0..UDP_PAYLOAD_SIZE * 2 + 37)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();

        assert_eq!(
            send_udp_bytes(&sender, destination, &payload).unwrap(),
            payload.len() as u64
        );

        let mut received = Vec::new();
        let mut datagram = [0u8; UDP_PAYLOAD_SIZE + 1];
        while received.len() < payload.len() {
            let (len, _) = receiver.recv_from(&mut datagram).unwrap();
            assert!(len <= UDP_PAYLOAD_SIZE);
            received.extend_from_slice(&datagram[..len]);
        }
        assert_eq!(received, payload);
    }

    #[test]
    fn send_cached_keyframe_writes_the_full_access_unit() {
        use std::io::Read;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let packets = vec![vec![0, 0, 0, 1, 0x67, 1], vec![0, 0, 0, 1, 0x65, 2, 3]];
        let client = std::thread::spawn(move || {
            let mut stream = TcpStream::connect(addr).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let mut buf = Vec::new();
            let _ = stream.read_to_end(&mut buf);
            buf
        });
        let (stream, _) = listener.accept().unwrap();
        send_cached_keyframe(&stream, Some(&packets)).unwrap();
        drop(stream);
        assert_eq!(
            client.join().unwrap(),
            [0, 0, 0, 1, 0x67, 1, 0, 0, 0, 1, 0x65, 2, 3]
        );
    }

    #[test]
    fn caches_hevc_vps_sps_and_pps() {
        let packet = [
            0,
            0,
            0,
            1,
            annexb::HEVC_VPS << 1,
            1,
            0,
            0,
            1,
            annexb::HEVC_SPS << 1,
            1,
            0,
            0,
            0,
            1,
            annexb::HEVC_PPS << 1,
            1,
        ];
        let mut sets = ParameterSets::default();
        update_parameter_sets(&packet, StreamCodec::Hevc, &mut sets);
        assert!(sets.vps.is_some());
        assert!(sets.sps.is_some());
        assert!(sets.pps.is_some());
    }
}
