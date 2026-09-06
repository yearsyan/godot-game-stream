// SPDX-License-Identifier: LGPL-2.1-or-later
// Copyright (C) 2026 yearsyan and contributors

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use godot::prelude::*;

pub const DEFAULT_OUTPUT_WIDTH: u32 = 1280;
pub const DEFAULT_OUTPUT_HEIGHT: u32 = 720;
pub const DEFAULT_TARGET_FPS: u32 = 60;
pub const MAX_TARGET_FPS: u32 = 120;
pub const MAX_IN_FLIGHT: u32 = 2;
// Bandwidth is intentionally traded for quality: 10 Mbps at 720p60 scales to
// 22.5 Mbps at 1080p60, 40 Mbps at 1440p60, and 90 Mbps at 4K60.
pub const BASE_TARGET_BITRATE: usize = 10_000_000;
pub const VIDEO_TIME_BASE: i32 = 90_000;
pub const DOWNSCALE_BUFFER_COUNT: usize = 2;
const FRAME_POOL_CAPACITY: usize = MAX_IN_FLIGHT as usize + 2;
const TIMING_WINDOW_SECONDS: usize = 10;
const MIN_TARGET_BITRATE: usize = 500_000;
const MAX_TARGET_BITRATE: usize = 200_000_000;

/// Requested or selected wire codec. `Auto` is only valid before the encoder
/// is opened; every encoded frame carries one concrete codec.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StreamCodec {
    Auto,
    Av1,
    Hevc,
    H264,
}

impl StreamCodec {
    pub fn from_env_value(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "av1" => Some(Self::Av1),
            "hevc" | "h265" => Some(Self::Hevc),
            "h264" | "avc" => Some(Self::H264),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Av1 => "av1",
            Self::Hevc => "hevc",
            Self::H264 => "h264",
        }
    }

    pub fn is_concrete(self) -> bool {
        self != Self::Auto
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamConfig {
    pub requested_width: u32,
    pub requested_height: u32,
    pub fps: u32,
    pub codec: StreamCodec,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            requested_width: DEFAULT_OUTPUT_WIDTH,
            requested_height: DEFAULT_OUTPUT_HEIGHT,
            fps: DEFAULT_TARGET_FPS,
            codec: StreamCodec::H264,
        }
    }
}

impl StreamConfig {
    pub fn new(width: u32, height: u32, fps: u32) -> Option<Self> {
        Self::with_codec(width, height, fps, StreamCodec::H264)
    }

    pub fn with_codec(width: u32, height: u32, fps: u32, codec: StreamCodec) -> Option<Self> {
        (width >= 2 && height >= 2 && (1..=MAX_TARGET_FPS).contains(&fps)).then_some(Self {
            requested_width: width,
            requested_height: height,
            fps,
            codec,
        })
    }

    pub fn dimensions_for_source(
        self,
        source_width: u32,
        source_height: u32,
    ) -> Option<(u32, u32)> {
        let width = self.requested_width.min(source_width) & !1;
        let height = self.requested_height.min(source_height) & !1;
        (width >= 2 && height >= 2).then_some((width, height))
    }

    pub fn bitrate_for(self, width: u32, height: u32) -> usize {
        let numerator = (BASE_TARGET_BITRATE as u128)
            .saturating_mul(width as u128)
            .saturating_mul(height as u128)
            .saturating_mul(self.fps as u128);
        let denominator = (DEFAULT_OUTPUT_WIDTH as u128)
            * (DEFAULT_OUTPUT_HEIGHT as u128)
            * (DEFAULT_TARGET_FPS as u128);
        let bitrate = numerator.div_ceil(denominator).min(usize::MAX as u128) as usize;
        bitrate.clamp(MIN_TARGET_BITRATE, MAX_TARGET_BITRATE)
    }

    fn timing_window_capacity(self) -> usize {
        self.fps as usize * TIMING_WINDOW_SECONDS
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum PixelFormat {
    Rgba8 = 1,
    Bgra8 = 2,
    Nv12 = 3,
}

impl PixelFormat {
    pub fn packed_bytes_per_pixel(self) -> Option<usize> {
        match self {
            Self::Rgba8 | Self::Bgra8 => Some(4),
            Self::Nv12 => None,
        }
    }

    pub fn from_u32(value: u32) -> Option<Self> {
        match value {
            1 => Some(Self::Rgba8),
            2 => Some(Self::Bgra8),
            3 => Some(Self::Nv12),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rgba8 => "rgba8",
            Self::Bgra8 => "bgra8",
            Self::Nv12 => "nv12",
        }
    }
}

pub struct CapturedFrame {
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub row_stride: u32,
    pub output_width: u32,
    pub output_height: u32,
    pub format: PixelFormat,
    pub sequence: u64,
    pub timeline: CaptureTimeline,
    pub pts: i64,
}

#[derive(Clone, Copy)]
pub struct CaptureTimeline {
    pub capture_unix_us: u64,
    pub post_draw_at: Instant,
    pub render_thread_at: Instant,
    pub readback_submitted_at: Instant,
    pub callback_at: Instant,
    pub ready_for_encode_at: Instant,
}

pub struct EncodedFrame {
    pub packets: Vec<Vec<u8>>,
    /// Concrete codec of this access unit. It is attached to the frame so a
    /// reconfigure cannot race the network thread's codec-dependent handling.
    pub codec: StreamCodec,
    /// Whether this access unit is a decoder-reset point, taken from the
    /// encoder packet flags so the net thread never parses slices.
    pub keyframe: bool,
    pub sequence: u64,
    pub timeline: CaptureTimeline,
    pub encode_started_at: Instant,
    pub encoded_at: Instant,
    pub scale_us: u64,
    pub encode_us: u64,
}

pub struct Mailbox {
    slot: Mutex<Option<CapturedFrame>>,
    cond: Condvar,
}

impl Mailbox {
    fn new() -> Self {
        Self {
            slot: Mutex::new(None),
            cond: Condvar::new(),
        }
    }

    pub fn push(&self, frame: CapturedFrame) -> Option<CapturedFrame> {
        let mut slot = self.slot.lock().expect("mailbox mutex");
        let dropped = slot.replace(frame);
        self.cond.notify_one();
        dropped
    }

    pub fn wait_take(&self, running: &AtomicBool) -> Option<CapturedFrame> {
        let mut slot = self.slot.lock().expect("mailbox mutex");
        loop {
            if !running.load(Ordering::SeqCst) {
                return None;
            }
            if let Some(frame) = slot.take() {
                return Some(frame);
            }
            let (guard, _) = self
                .cond
                .wait_timeout(slot, Duration::from_millis(50))
                .expect("mailbox condvar");
            slot = guard;
        }
    }

    pub fn notify(&self) {
        self.cond.notify_all();
    }
}

pub struct EncodedMailbox {
    slot: Mutex<Option<EncodedFrame>>,
    cond: Condvar,
}

impl EncodedMailbox {
    fn new() -> Self {
        Self {
            slot: Mutex::new(None),
            cond: Condvar::new(),
        }
    }

    pub fn push(&self, frame: EncodedFrame) -> bool {
        let mut slot = self.slot.lock().expect("encoded mailbox mutex");
        let dropped = slot.replace(frame).is_some();
        self.cond.notify_one();
        dropped
    }

    pub fn wait_take(&self, running: &AtomicBool, timeout: Duration) -> Option<EncodedFrame> {
        let mut slot = self.slot.lock().expect("encoded mailbox mutex");
        if let Some(frame) = slot.take() {
            return Some(frame);
        }
        if !running.load(Ordering::SeqCst) {
            return None;
        }
        let (mut slot, _) = self
            .cond
            .wait_timeout(slot, timeout)
            .expect("encoded mailbox condvar");
        slot.take()
    }

    fn notify(&self) {
        self.cond.notify_all();
    }
}

pub struct FrameBufferPool {
    buffers: Mutex<Vec<Vec<u8>>>,
}

impl Default for FrameBufferPool {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameBufferPool {
    pub fn new() -> Self {
        Self {
            buffers: Mutex::new(Vec::with_capacity(FRAME_POOL_CAPACITY)),
        }
    }

    pub fn take(&self, len: usize) -> Vec<u8> {
        let mut buffers = self.buffers.lock().expect("frame pool mutex");
        let best = buffers
            .iter()
            .enumerate()
            .filter(|(_, buffer)| buffer.capacity() >= len)
            .min_by_key(|(_, buffer)| buffer.capacity())
            .map(|(index, _)| index);
        let mut buffer = best
            .map(|index| buffers.swap_remove(index))
            .unwrap_or_else(|| Vec::with_capacity(len));
        buffer.resize(len, 0);
        buffer
    }

    pub fn recycle(&self, mut buffer: Vec<u8>) {
        buffer.clear();
        let mut buffers = self.buffers.lock().expect("frame pool mutex");
        if buffers.len() < FRAME_POOL_CAPACITY {
            buffers.push(buffer);
        }
    }

    #[cfg(test)]
    pub(crate) fn available(&self) -> usize {
        self.buffers.lock().expect("frame pool mutex").len()
    }
}

pub struct SubmitClock {
    next_due: Option<Instant>,
}

impl SubmitClock {
    fn new() -> Self {
        Self { next_due: None }
    }

    pub fn should_submit(&mut self, now: Instant, interval: Duration) -> bool {
        let Some(next_due) = self.next_due else {
            self.next_due = Some(now + interval);
            return true;
        };
        if now < next_due {
            return false;
        }

        let lateness = now.saturating_duration_since(next_due);
        self.next_due = Some(if lateness >= interval {
            now + interval
        } else {
            next_due + interval
        });
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum EncoderBackend {
    None = 0,
    Av1Nvenc = 1,
    HevcNvenc = 2,
    H264Nvenc = 3,
    Av1Amf = 4,
    HevcAmf = 5,
    H264Amf = 6,
    Av1Qsv = 7,
    HevcQsv = 8,
    H264Qsv = 9,
    HevcVideoToolbox = 10,
    H264VideoToolbox = 11,
}

impl EncoderBackend {
    pub fn from_u32(value: u32) -> Self {
        match value {
            1 => Self::Av1Nvenc,
            2 => Self::HevcNvenc,
            3 => Self::H264Nvenc,
            4 => Self::Av1Amf,
            5 => Self::HevcAmf,
            6 => Self::H264Amf,
            7 => Self::Av1Qsv,
            8 => Self::HevcQsv,
            9 => Self::H264Qsv,
            10 => Self::HevcVideoToolbox,
            11 => Self::H264VideoToolbox,
            _ => Self::None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Av1Nvenc => "av1_nvenc",
            Self::HevcNvenc => "hevc_nvenc",
            Self::H264Nvenc => "h264_nvenc",
            Self::Av1Amf => "av1_amf",
            Self::HevcAmf => "hevc_amf",
            Self::H264Amf => "h264_amf",
            Self::Av1Qsv => "av1_qsv",
            Self::HevcQsv => "hevc_qsv",
            Self::H264Qsv => "h264_qsv",
            Self::HevcVideoToolbox => "hevc_videotoolbox",
            Self::H264VideoToolbox => "h264_videotoolbox",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuReadbackKind {
    Nv12Buffer { row_stride: u32, size_bytes: u32 },
    RgbaTexture,
}

pub struct DownscaleResources {
    pub output_width: u32,
    pub output_height: u32,
    pub readback_kind: GpuReadbackKind,
    pub shader: Rid,
    pub pipeline: Rid,
    pub sampler: Rid,
    pub output_resources: [Rid; DOWNSCALE_BUFFER_COUNT],
    pub uniform_sets: [Rid; DOWNSCALE_BUFFER_COUNT],
    pub source_textures: [Rid; DOWNSCALE_BUFFER_COUNT],
    pub busy: [bool; DOWNSCALE_BUFFER_COUNT],
    pub next_slot: usize,
}

pub enum DownscaleState {
    Uninitialized,
    Ready(DownscaleResources),
    Disabled,
}

#[derive(Clone, Copy)]
pub struct FrameTimingSample {
    pub captured_at: Instant,
    pub completed_at: Instant,
    pub post_to_render_us: u64,
    pub render_setup_us: u64,
    pub readback_wait_us: u64,
    pub callback_copy_us: u64,
    pub encode_queue_us: u64,
    pub scale_us: u64,
    pub encode_us: u64,
    pub encoder_stage_us: u64,
    pub capture_to_encoded_us: u64,
    pub network_queue_us: u64,
    pub packet_prep_us: u64,
    pub socket_write_us: u64,
    pub total_us: u64,
    pub frame_bytes: u64,
}

#[derive(Clone, Copy, Default)]
pub struct TimingPercentiles {
    pub p50_us: u64,
    pub p95_us: u64,
    pub p99_us: u64,
    pub max_us: u64,
}

impl TimingPercentiles {
    fn as_dictionary(self) -> VarDictionary {
        let mut stats = VarDictionary::new();
        stats.set("p50", self.p50_us as i64);
        stats.set("p95", self.p95_us as i64);
        stats.set("p99", self.p99_us as i64);
        stats.set("max", self.max_us as i64);
        stats
    }
}

#[derive(Clone, Copy, Default)]
pub struct TimingSummary {
    pub samples: u64,
    pub window_ms: u64,
    pub delivered_fps: f64,
    pub stream_bitrate_bps: u64,
    pub post_to_render: TimingPercentiles,
    pub render_setup: TimingPercentiles,
    pub readback_wait: TimingPercentiles,
    pub callback_copy: TimingPercentiles,
    pub encode_queue: TimingPercentiles,
    pub scale: TimingPercentiles,
    pub encode: TimingPercentiles,
    pub encoder_stage: TimingPercentiles,
    pub capture_to_encoded: TimingPercentiles,
    pub network_queue: TimingPercentiles,
    pub packet_prep: TimingPercentiles,
    pub socket_write: TimingPercentiles,
    pub total: TimingPercentiles,
}

impl TimingSummary {
    pub fn as_dictionary(self) -> VarDictionary {
        let mut stats = VarDictionary::new();
        stats.set("samples", self.samples as i64);
        stats.set("window_ms", self.window_ms as i64);
        stats.set("delivered_fps", self.delivered_fps);
        stats.set("stream_bitrate_bps", self.stream_bitrate_bps as i64);
        stats.set("post_to_render_us", &self.post_to_render.as_dictionary());
        stats.set("render_setup_us", &self.render_setup.as_dictionary());
        stats.set("readback_wait_us", &self.readback_wait.as_dictionary());
        stats.set("callback_copy_us", &self.callback_copy.as_dictionary());
        stats.set("encode_queue_us", &self.encode_queue.as_dictionary());
        stats.set("scale_us", &self.scale.as_dictionary());
        stats.set("encode_us", &self.encode.as_dictionary());
        stats.set("encoder_stage_us", &self.encoder_stage.as_dictionary());
        stats.set(
            "capture_to_encoded_us",
            &self.capture_to_encoded.as_dictionary(),
        );
        stats.set("network_queue_us", &self.network_queue.as_dictionary());
        stats.set("packet_prep_us", &self.packet_prep.as_dictionary());
        stats.set("socket_write_us", &self.socket_write.as_dictionary());
        stats.set("capture_to_socket_us", &self.total.as_dictionary());
        stats
    }
}

pub struct TimingWindow {
    samples: VecDeque<FrameTimingSample>,
    capacity: usize,
}

impl TimingWindow {
    fn new(capacity: usize) -> Self {
        Self {
            samples: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    fn record(&mut self, sample: FrameTimingSample) {
        if self.samples.len() == self.capacity {
            self.samples.pop_front();
        }
        self.samples.push_back(sample);
    }

    fn summary(&self) -> TimingSummary {
        let Some(first) = self.samples.front() else {
            return TimingSummary::default();
        };
        let last = self.samples.back().expect("non-empty timing window");
        let duration = last
            .completed_at
            .saturating_duration_since(first.captured_at);
        let duration_seconds = duration.as_secs_f64();
        let bytes = self
            .samples
            .iter()
            .map(|sample| sample.frame_bytes)
            .sum::<u64>();

        TimingSummary {
            samples: self.samples.len() as u64,
            window_ms: duration.as_millis() as u64,
            delivered_fps: if duration_seconds > 0.0 {
                self.samples.len() as f64 / duration_seconds
            } else {
                0.0
            },
            stream_bitrate_bps: if duration_seconds > 0.0 {
                ((bytes as f64 * 8.0) / duration_seconds) as u64
            } else {
                0
            },
            post_to_render: self.percentiles(|sample| sample.post_to_render_us),
            render_setup: self.percentiles(|sample| sample.render_setup_us),
            readback_wait: self.percentiles(|sample| sample.readback_wait_us),
            callback_copy: self.percentiles(|sample| sample.callback_copy_us),
            encode_queue: self.percentiles(|sample| sample.encode_queue_us),
            scale: self.percentiles(|sample| sample.scale_us),
            encode: self.percentiles(|sample| sample.encode_us),
            encoder_stage: self.percentiles(|sample| sample.encoder_stage_us),
            capture_to_encoded: self.percentiles(|sample| sample.capture_to_encoded_us),
            network_queue: self.percentiles(|sample| sample.network_queue_us),
            packet_prep: self.percentiles(|sample| sample.packet_prep_us),
            socket_write: self.percentiles(|sample| sample.socket_write_us),
            total: self.percentiles(|sample| sample.total_us),
        }
    }

    fn percentiles(&self, value: impl Fn(&FrameTimingSample) -> u64) -> TimingPercentiles {
        let mut values = self.samples.iter().map(value).collect::<Vec<_>>();
        if values.is_empty() {
            return TimingPercentiles::default();
        }
        values.sort_unstable();
        TimingPercentiles {
            p50_us: percentile(&values, 50),
            p95_us: percentile(&values, 95),
            p99_us: percentile(&values, 99),
            max_us: *values.last().expect("non-empty percentile values"),
        }
    }
}

fn percentile(sorted: &[u64], percent: usize) -> u64 {
    let rank = sorted
        .len()
        .saturating_mul(percent)
        .div_ceil(100)
        .saturating_sub(1)
        .min(sorted.len() - 1);
    sorted[rank]
}

pub struct SharedState {
    pub config: StreamConfig,
    pub running: AtomicBool,
    pub generation: AtomicU64,
    pub in_flight: AtomicU32,
    /// When `in_flight` last saturated at MAX_IN_FLIGHT (None = healthy).
    /// A stale timestamp means readback completions stopped arriving and the
    /// submit gate must self-heal instead of freezing the stream forever.
    pub in_flight_stuck_since: Mutex<Option<Instant>>,
    pub submitted: AtomicU64,
    pub completed: AtomicU64,
    pub throttled: AtomicU64,
    pub dropped_inflight: AtomicU64,
    pub dropped_before_encode: AtomicU64,
    pub dropped_after_encode: AtomicU64,
    pub dropped_no_client: AtomicU64,
    pub encode_ok: AtomicU64,
    pub encoder_no_output: AtomicU64,
    pub encoder_direct_frames: AtomicU64,
    pub bytes_sent: AtomicU64,
    pub readback_bytes: AtomicU64,
    pub last_readback_bytes: AtomicU64,
    pub post_to_render_us: AtomicU64,
    pub render_setup_us: AtomicU64,
    pub callback_delay_us: AtomicU64,
    pub callback_copy_us: AtomicU64,
    pub encode_queue_us: AtomicU64,
    pub scale_us: AtomicU64,
    pub encode_us: AtomicU64,
    pub encoder_stage_us: AtomicU64,
    pub capture_to_encoded_us: AtomicU64,
    pub network_queue_us: AtomicU64,
    pub packet_prep_us: AtomicU64,
    pub socket_write_us: AtomicU64,
    pub capture_to_socket_us: AtomicU64,
    pub last_access_unit_bytes: AtomicU64,
    pub last_width: AtomicU32,
    pub last_height: AtomicU32,
    pub last_readback_width: AtomicU32,
    pub last_readback_height: AtomicU32,
    pub last_readback_format: AtomicU32,
    pub output_width: AtomicU32,
    pub output_height: AtomicU32,
    pub target_bitrate: AtomicU64,
    pub client_count: AtomicU32,
    pub force_keyframe: AtomicBool,
    /// Unix-µs timestamps (0 = never) feeding the draw watchdog: the last
    /// frame_post_draw seen and the last redraw the watchdog forced itself.
    pub last_post_draw_us: AtomicU64,
    pub last_forced_draw_us: AtomicU64,
    pub sequence: AtomicU64,
    pub encoder_backend: AtomicU32,
    pub gpu_downscale_active: AtomicBool,
    pub gpu_downscale_frames: AtomicU64,
    pub gpu_nv12_frames: AtomicU64,
    pub gpu_downscale_fallbacks: AtomicU64,
    pub cleanup_requested: AtomicBool,
    pub input_enabled: bool,
    pub input_events_received: AtomicU64,
    pub input_events_injected: AtomicU64,
    pub input_events_dropped: AtomicU64,
    pub submit_clock: Mutex<SubmitClock>,
    pub viewport: Mutex<Rid>,
    /// Codec actually on the wire. It remains `Auto` until probing succeeds.
    pub active_codec: Mutex<StreamCodec>,
    pub mailbox: Mailbox,
    pub encoded_mailbox: EncodedMailbox,
    pub frame_pool: Arc<FrameBufferPool>,
    pub downscale: Mutex<DownscaleState>,
    pub timing_window: Mutex<TimingWindow>,
    pub input_queue: crate::input::InputQueue,
    pub started: Instant,
}

impl SharedState {
    pub fn new(viewport: Rid, config: StreamConfig, input_enabled: bool) -> Arc<Self> {
        Arc::new(Self {
            config,
            running: AtomicBool::new(true),
            generation: AtomicU64::new(1),
            in_flight: AtomicU32::new(0),
            in_flight_stuck_since: Mutex::new(None),
            submitted: AtomicU64::new(0),
            completed: AtomicU64::new(0),
            throttled: AtomicU64::new(0),
            dropped_inflight: AtomicU64::new(0),
            dropped_before_encode: AtomicU64::new(0),
            dropped_after_encode: AtomicU64::new(0),
            dropped_no_client: AtomicU64::new(0),
            encode_ok: AtomicU64::new(0),
            encoder_no_output: AtomicU64::new(0),
            encoder_direct_frames: AtomicU64::new(0),
            bytes_sent: AtomicU64::new(0),
            readback_bytes: AtomicU64::new(0),
            last_readback_bytes: AtomicU64::new(0),
            post_to_render_us: AtomicU64::new(0),
            render_setup_us: AtomicU64::new(0),
            callback_delay_us: AtomicU64::new(0),
            callback_copy_us: AtomicU64::new(0),
            encode_queue_us: AtomicU64::new(0),
            scale_us: AtomicU64::new(0),
            encode_us: AtomicU64::new(0),
            encoder_stage_us: AtomicU64::new(0),
            capture_to_encoded_us: AtomicU64::new(0),
            network_queue_us: AtomicU64::new(0),
            packet_prep_us: AtomicU64::new(0),
            socket_write_us: AtomicU64::new(0),
            capture_to_socket_us: AtomicU64::new(0),
            last_access_unit_bytes: AtomicU64::new(0),
            last_width: AtomicU32::new(0),
            last_height: AtomicU32::new(0),
            last_readback_width: AtomicU32::new(0),
            last_readback_height: AtomicU32::new(0),
            last_readback_format: AtomicU32::new(0),
            output_width: AtomicU32::new(0),
            output_height: AtomicU32::new(0),
            target_bitrate: AtomicU64::new(0),
            client_count: AtomicU32::new(0),
            force_keyframe: AtomicBool::new(true),
            last_post_draw_us: AtomicU64::new(0),
            last_forced_draw_us: AtomicU64::new(0),
            sequence: AtomicU64::new(0),
            encoder_backend: AtomicU32::new(EncoderBackend::None as u32),
            gpu_downscale_active: AtomicBool::new(false),
            gpu_downscale_frames: AtomicU64::new(0),
            gpu_nv12_frames: AtomicU64::new(0),
            gpu_downscale_fallbacks: AtomicU64::new(0),
            cleanup_requested: AtomicBool::new(false),
            input_enabled,
            input_events_received: AtomicU64::new(0),
            input_events_injected: AtomicU64::new(0),
            input_events_dropped: AtomicU64::new(0),
            submit_clock: Mutex::new(SubmitClock::new()),
            viewport: Mutex::new(viewport),
            // The requested codec lives in `config`; the active codec is not
            // known until a concrete encoder context opens successfully.
            active_codec: Mutex::new(StreamCodec::Auto),
            mailbox: Mailbox::new(),
            encoded_mailbox: EncodedMailbox::new(),
            frame_pool: Arc::new(FrameBufferPool::new()),
            downscale: Mutex::new(DownscaleState::Uninitialized),
            timing_window: Mutex::new(TimingWindow::new(config.timing_window_capacity())),
            input_queue: crate::input::InputQueue::new(),
            started: Instant::now(),
        })
    }

    pub fn request_stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.mailbox.notify();
        self.encoded_mailbox.notify();
    }

    pub fn encoder_backend(&self) -> EncoderBackend {
        EncoderBackend::from_u32(self.encoder_backend.load(Ordering::Relaxed))
    }

    pub fn record_timing(&self, sample: FrameTimingSample) {
        self.timing_window
            .lock()
            .expect("timing window mutex")
            .record(sample);
    }

    pub fn timing_summary(&self) -> TimingSummary {
        self.timing_window
            .lock()
            .expect("timing window mutex")
            .summary()
    }

    pub fn snapshot(&self) -> VarDictionary {
        let mut stats = VarDictionary::new();
        stats.set("submitted", self.submitted.load(Ordering::Relaxed) as i64);
        stats.set("completed", self.completed.load(Ordering::Relaxed) as i64);
        stats.set("throttled", self.throttled.load(Ordering::Relaxed) as i64);
        stats.set("in_flight", self.in_flight.load(Ordering::Relaxed) as i64);
        stats.set(
            "dropped_inflight",
            self.dropped_inflight.load(Ordering::Relaxed) as i64,
        );
        stats.set(
            "dropped_before_encode",
            self.dropped_before_encode.load(Ordering::Relaxed) as i64,
        );
        stats.set(
            "dropped_after_encode",
            self.dropped_after_encode.load(Ordering::Relaxed) as i64,
        );
        stats.set(
            "dropped_no_client",
            self.dropped_no_client.load(Ordering::Relaxed) as i64,
        );
        stats.set("encode_ok", self.encode_ok.load(Ordering::Relaxed) as i64);
        stats.set(
            "encoder_no_output",
            self.encoder_no_output.load(Ordering::Relaxed) as i64,
        );
        stats.set(
            "encoder_direct_frames",
            self.encoder_direct_frames.load(Ordering::Relaxed) as i64,
        );
        stats.set("bytes_sent", self.bytes_sent.load(Ordering::Relaxed) as i64);
        stats.set(
            "readback_bytes",
            self.readback_bytes.load(Ordering::Relaxed) as i64,
        );
        stats.set(
            "last_readback_bytes",
            self.last_readback_bytes.load(Ordering::Relaxed) as i64,
        );
        stats.set(
            "post_to_render_us",
            self.post_to_render_us.load(Ordering::Relaxed) as i64,
        );
        stats.set(
            "render_setup_us",
            self.render_setup_us.load(Ordering::Relaxed) as i64,
        );
        stats.set(
            "callback_delay_us",
            self.callback_delay_us.load(Ordering::Relaxed) as i64,
        );
        stats.set(
            "callback_copy_us",
            self.callback_copy_us.load(Ordering::Relaxed) as i64,
        );
        stats.set(
            "encode_queue_us",
            self.encode_queue_us.load(Ordering::Relaxed) as i64,
        );
        stats.set("scale_us", self.scale_us.load(Ordering::Relaxed) as i64);
        stats.set("encode_us", self.encode_us.load(Ordering::Relaxed) as i64);
        stats.set(
            "encoder_stage_us",
            self.encoder_stage_us.load(Ordering::Relaxed) as i64,
        );
        stats.set(
            "capture_to_encoded_us",
            self.capture_to_encoded_us.load(Ordering::Relaxed) as i64,
        );
        stats.set(
            "network_queue_us",
            self.network_queue_us.load(Ordering::Relaxed) as i64,
        );
        stats.set(
            "packet_prep_us",
            self.packet_prep_us.load(Ordering::Relaxed) as i64,
        );
        stats.set(
            "socket_write_us",
            self.socket_write_us.load(Ordering::Relaxed) as i64,
        );
        stats.set(
            "capture_to_socket_us",
            self.capture_to_socket_us.load(Ordering::Relaxed) as i64,
        );
        stats.set(
            "last_access_unit_bytes",
            self.last_access_unit_bytes.load(Ordering::Relaxed) as i64,
        );
        stats.set("timing", &self.timing_summary().as_dictionary());
        stats.set("last_width", self.last_width.load(Ordering::Relaxed) as i64);
        stats.set(
            "last_height",
            self.last_height.load(Ordering::Relaxed) as i64,
        );
        stats.set(
            "last_readback_width",
            self.last_readback_width.load(Ordering::Relaxed) as i64,
        );
        stats.set(
            "last_readback_height",
            self.last_readback_height.load(Ordering::Relaxed) as i64,
        );
        let readback_format =
            PixelFormat::from_u32(self.last_readback_format.load(Ordering::Relaxed))
                .map_or("none", PixelFormat::as_str);
        stats.set("last_readback_format", readback_format);
        stats.set("requested_width", self.config.requested_width as i64);
        stats.set("requested_height", self.config.requested_height as i64);
        stats.set("target_fps", self.config.fps as i64);
        stats.set(
            "output_width",
            self.output_width.load(Ordering::Relaxed) as i64,
        );
        stats.set(
            "output_height",
            self.output_height.load(Ordering::Relaxed) as i64,
        );
        stats.set(
            "target_bitrate_bps",
            self.target_bitrate.load(Ordering::Relaxed) as i64,
        );
        stats.set(
            "client_count",
            self.client_count.load(Ordering::Relaxed) as i64,
        );
        stats.set("running", self.running.load(Ordering::SeqCst));
        stats.set("encoder_backend", self.encoder_backend().as_str());
        stats.set("requested_codec", self.config.codec.as_str());
        stats.set(
            "active_codec",
            self.active_codec
                .lock()
                .expect("active codec mutex")
                .as_str(),
        );
        stats.set(
            "gpu_downscale_active",
            self.gpu_downscale_active.load(Ordering::Relaxed),
        );
        stats.set(
            "gpu_downscale_frames",
            self.gpu_downscale_frames.load(Ordering::Relaxed) as i64,
        );
        stats.set(
            "gpu_nv12_frames",
            self.gpu_nv12_frames.load(Ordering::Relaxed) as i64,
        );
        stats.set(
            "gpu_downscale_fallbacks",
            self.gpu_downscale_fallbacks.load(Ordering::Relaxed) as i64,
        );
        stats.set("input_enabled", self.input_enabled);
        stats.set(
            "input_received",
            self.input_events_received.load(Ordering::Relaxed) as i64,
        );
        stats.set(
            "input_injected",
            self.input_events_injected.load(Ordering::Relaxed) as i64,
        );
        stats.set(
            "input_dropped",
            self.input_events_dropped.load(Ordering::Relaxed) as i64,
        );
        stats.set("uptime_ms", self.started.elapsed().as_millis() as i64);
        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn submit_clock_does_not_halve_a_slightly_fast_sixty_hz_source() {
        let start = Instant::now();
        let interval = Duration::from_nanos(1_000_000_000 / 60);
        let source_interval = Duration::from_micros(16_500);
        let mut clock = SubmitClock::new();
        let accepted = (0..600)
            .filter(|frame| clock.should_submit(start + source_interval * *frame, interval))
            .count();

        assert!(accepted >= 590, "accepted only {accepted} frames");
    }

    #[test]
    fn submit_clock_samples_seventy_five_hz_close_to_sixty_hz() {
        let start = Instant::now();
        let interval = Duration::from_nanos(1_000_000_000 / 60);
        let source_interval = Duration::from_nanos(1_000_000_000 / 75);
        let mut clock = SubmitClock::new();
        let accepted = (0..750)
            .filter(|frame| clock.should_submit(start + source_interval * *frame, interval))
            .count();

        assert!(
            (598..=602).contains(&accepted),
            "accepted {accepted} frames"
        );
    }

    #[test]
    fn timing_window_reports_nearest_rank_percentiles() {
        let start = Instant::now();
        let mut window = TimingWindow::new(100);
        for value in 1..=100_u64 {
            let captured_at = start + Duration::from_millis(value);
            window.record(FrameTimingSample {
                captured_at,
                completed_at: captured_at + Duration::from_millis(1),
                post_to_render_us: value,
                render_setup_us: value,
                readback_wait_us: value,
                callback_copy_us: value,
                encode_queue_us: value,
                scale_us: value,
                encode_us: value,
                encoder_stage_us: value,
                capture_to_encoded_us: value,
                network_queue_us: value,
                packet_prep_us: value,
                socket_write_us: value,
                total_us: value,
                frame_bytes: 1_000,
            });
        }

        let summary = window.summary();
        assert_eq!(summary.samples, 100);
        assert_eq!(summary.total.p50_us, 50);
        assert_eq!(summary.total.p95_us, 95);
        assert_eq!(summary.total.p99_us, 99);
        assert_eq!(summary.total.max_us, 100);
        assert!(summary.stream_bitrate_bps > 0);
    }

    #[test]
    fn stream_config_clamps_to_even_source_dimensions() {
        let config = StreamConfig::new(1920, 1080, 120).expect("valid config");
        assert_eq!(config.dimensions_for_source(2560, 1181), Some((1920, 1080)));
        assert_eq!(config.dimensions_for_source(1281, 721), Some((1280, 720)));
        assert_eq!(config.bitrate_for(1920, 1080), 45_000_000);
    }

    #[test]
    fn stream_config_rejects_fps_above_one_twenty() {
        assert!(StreamConfig::new(1920, 1080, 120).is_some());
        assert!(StreamConfig::new(1920, 1080, 121).is_none());
    }
}
