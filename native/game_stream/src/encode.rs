// SPDX-License-Identifier: LGPL-2.1-or-later
// Copyright (C) 2026 yearsyan and contributors

use std::ffi::c_void;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use ffmpeg_next as ffmpeg;
use ffmpeg_next::format::Pixel;
use ffmpeg_next::frame::Video as VideoFrame;
use ffmpeg_next::software::scaling::{Context as SwsContext, Flags as SwsFlags};
use ffmpeg_next::util::color::{Primaries, Range, Space, TransferCharacteristic};
use ffmpeg_next::{codec, encoder, picture, Dictionary, Packet, Rational};

use crate::shared::{
    CapturedFrame, EncodedFrame, EncoderBackend, FrameBufferPool, PixelFormat, SharedState,
    StreamCodec, StreamConfig, VIDEO_TIME_BASE,
};

const TELEMETRY_UUID: [u8; 16] = *b"GSTREAM-TIMING01";
const AV1_TEMPORAL_DELIMITER: [u8; 2] = [0x12, 0x00];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputPolicy {
    // Only the non-macOS candidate tables prefer the capture pixel format;
    // VideoToolbox wants NV12.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    Preferred,
    Nv12,
}

impl InputPolicy {
    fn pixel(self, preferred: PixelFormat) -> Pixel {
        match self {
            Self::Preferred => pixel_for(preferred),
            Self::Nv12 => Pixel::NV12,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EncoderCandidate {
    name: &'static str,
    codec: StreamCodec,
    backend: EncoderBackend,
    input: InputPolicy,
}

#[cfg(not(target_os = "macos"))]
const ENCODER_CANDIDATES: &[EncoderCandidate] = &[
    // Codec priority is intentional and takes precedence over vendor. Opening
    // the codec context is the capability probe: a compiled wrapper alone does
    // not prove that the installed GPU can encode the format.
    EncoderCandidate {
        name: "av1_nvenc",
        codec: StreamCodec::Av1,
        backend: EncoderBackend::Av1Nvenc,
        input: InputPolicy::Preferred,
    },
    EncoderCandidate {
        name: "av1_amf",
        codec: StreamCodec::Av1,
        backend: EncoderBackend::Av1Amf,
        input: InputPolicy::Preferred,
    },
    EncoderCandidate {
        name: "av1_qsv",
        codec: StreamCodec::Av1,
        backend: EncoderBackend::Av1Qsv,
        input: InputPolicy::Nv12,
    },
    EncoderCandidate {
        name: "hevc_nvenc",
        codec: StreamCodec::Hevc,
        backend: EncoderBackend::HevcNvenc,
        input: InputPolicy::Preferred,
    },
    EncoderCandidate {
        name: "hevc_amf",
        codec: StreamCodec::Hevc,
        backend: EncoderBackend::HevcAmf,
        input: InputPolicy::Preferred,
    },
    EncoderCandidate {
        name: "hevc_qsv",
        codec: StreamCodec::Hevc,
        backend: EncoderBackend::HevcQsv,
        input: InputPolicy::Nv12,
    },
    EncoderCandidate {
        name: "h264_nvenc",
        codec: StreamCodec::H264,
        backend: EncoderBackend::H264Nvenc,
        input: InputPolicy::Preferred,
    },
    EncoderCandidate {
        name: "h264_amf",
        codec: StreamCodec::H264,
        backend: EncoderBackend::H264Amf,
        input: InputPolicy::Preferred,
    },
    EncoderCandidate {
        name: "h264_qsv",
        codec: StreamCodec::H264,
        backend: EncoderBackend::H264Qsv,
        input: InputPolicy::Nv12,
    },
];

#[cfg(target_os = "macos")]
const ENCODER_CANDIDATES: &[EncoderCandidate] = &[
    // FFmpeg/VideoToolbox currently exposes HEVC and H.264 encoders, not AV1.
    EncoderCandidate {
        name: "hevc_videotoolbox",
        codec: StreamCodec::Hevc,
        backend: EncoderBackend::HevcVideoToolbox,
        input: InputPolicy::Nv12,
    },
    EncoderCandidate {
        name: "h264_videotoolbox",
        codec: StreamCodec::H264,
        backend: EncoderBackend::H264VideoToolbox,
        input: InputPolicy::Nv12,
    },
];

pub struct VideoEncoder {
    encoder: encoder::video::Encoder,
    selection: EncoderCandidate,
    input_pixel: Pixel,
    scaler: Option<SwsContext>,
    src: VideoFrame,
    converted: VideoFrame,
    output_width: u32,
    output_height: u32,
    last_pts: Option<i64>,
    last_input: Option<(u32, u32, PixelFormat)>,
}

#[derive(Clone, Copy)]
pub struct EncodeTiming {
    pub scale_us: u64,
    pub encode_us: u64,
    pub direct_input: bool,
}

impl VideoEncoder {
    fn new(
        config: StreamConfig,
        output_width: u32,
        output_height: u32,
        preferred_input: PixelFormat,
    ) -> Result<Self, ffmpeg::Error> {
        let mut last_error = ffmpeg::Error::EncoderNotFound;
        for &candidate in ENCODER_CANDIDATES {
            if config.codec != StreamCodec::Auto && candidate.codec != config.codec {
                continue;
            }

            match Self::open_candidate(
                config,
                candidate,
                output_width,
                output_height,
                preferred_input,
            ) {
                Ok(encoder) => return Ok(encoder),
                Err(error) => {
                    if config.codec == StreamCodec::Auto {
                        // Missing capabilities are expected while walking the
                        // auto list (for example AV1 NVENC on an RTX 3080).
                        godot::prelude::godot_print!(
                            "game_stream: encoder probe {} unavailable: {error}",
                            candidate.name
                        );
                    } else {
                        godot::prelude::godot_warn!(
                            "game_stream: requested encoder {} unavailable: {error}",
                            candidate.name
                        );
                    }
                    last_error = error;
                }
            }
        }
        Err(last_error)
    }

    fn open_candidate(
        config: StreamConfig,
        selection: EncoderCandidate,
        output_width: u32,
        output_height: u32,
        preferred_input: PixelFormat,
    ) -> Result<Self, ffmpeg::Error> {
        let input_pixel = selection.input.pixel(preferred_input);
        let bitrate = config.bitrate_for(output_width, output_height);
        let encoder = open_named_encoder(
            selection.name,
            input_pixel,
            output_width,
            output_height,
            config.fps,
            bitrate,
            encoder_options(selection, input_pixel),
        )?;

        Ok(Self {
            encoder,
            selection,
            input_pixel,
            scaler: None,
            src: VideoFrame::empty(),
            converted: VideoFrame::new(input_pixel, output_width, output_height),
            output_width,
            output_height,
            last_pts: None,
            last_input: None,
        })
    }

    pub fn backend(&self) -> EncoderBackend {
        self.selection.backend
    }

    pub fn codec(&self) -> StreamCodec {
        self.selection.codec
    }

    pub fn input_pixel(&self) -> Pixel {
        self.input_pixel
    }

    pub fn output_dimensions(&self) -> (u32, u32) {
        (self.output_width, self.output_height)
    }

    fn needs_reconfigure(
        &self,
        output_dimensions: (u32, u32),
        preferred_input: PixelFormat,
    ) -> bool {
        self.output_dimensions() != output_dimensions
            || self.selection.input.pixel(preferred_input) != self.input_pixel
    }

    pub fn encode(
        &mut self,
        frame: CapturedFrame,
        frame_pool: Arc<FrameBufferPool>,
        force_keyframe: bool,
        out: &mut Vec<(Vec<u8>, bool)>,
    ) -> Result<(EncodeTiming, bool), ffmpeg::Error> {
        if (frame.output_width, frame.output_height) != self.output_dimensions() {
            return Err(ffmpeg::Error::InvalidData);
        }
        // Async readbacks may complete with gaps. Keep capture-clock PTS while
        // guaranteeing the strictly increasing order required by encoders.
        let pts = self
            .last_pts
            .map_or(frame.pts, |previous| frame.pts.max(previous + 1));
        self.last_pts = Some(pts);

        let CapturedFrame {
            pixels,
            width,
            height,
            row_stride,
            format,
            ..
        } = frame;
        let pixels = PooledPixels::new(pixels, frame_pool);
        let direct_input = width == self.output_width
            && height == self.output_height
            && pixel_for(format) == self.input_pixel;

        let (scale_us, encode_us, keyframe) = if direct_input {
            validate_frame_layout(width, height, row_stride, format, pixels.len())?;
            let mut source = owned_video_frame(pixels, width, height, row_stride, format)?;
            set_frame_timing(&mut source, pts, force_keyframe);

            let encode_started = Instant::now();
            self.encoder.send_frame(&source)?;
            let keyframe = drain_packets(&mut self.encoder, out)?;
            (0, encode_started.elapsed().as_micros() as u64, keyframe)
        } else {
            self.ensure_scaler(width, height, format)?;
            let writable =
                unsafe { ffmpeg::ffi::av_frame_make_writable(self.converted.as_mut_ptr()) };
            if writable < 0 {
                return Err(ffmpeg::Error::from(writable));
            }

            attach_frame(
                &mut self.src,
                pixels.as_slice(),
                width,
                height,
                row_stride,
                format,
            )?;
            let scale_started = Instant::now();
            let scale_result = self
                .scaler
                .as_mut()
                .expect("scaler")
                .run(&self.src, &mut self.converted);
            let scale_us = scale_started.elapsed().as_micros() as u64;
            detach_frame(&mut self.src);
            scale_result?;

            set_frame_timing(&mut self.converted, pts, force_keyframe);
            let encode_started = Instant::now();
            self.encoder.send_frame(&self.converted)?;
            let keyframe = drain_packets(&mut self.encoder, out)?;
            (
                scale_us,
                encode_started.elapsed().as_micros() as u64,
                keyframe,
            )
        };

        if self.codec() == StreamCodec::Av1 && !out.is_empty() {
            add_av1_temporal_delimiters(out);
        }

        Ok((
            EncodeTiming {
                scale_us,
                encode_us,
                direct_input,
            },
            keyframe,
        ))
    }

    fn ensure_scaler(
        &mut self,
        width: u32,
        height: u32,
        format: PixelFormat,
    ) -> Result<(), ffmpeg::Error> {
        let input = (width, height, format);
        if self.last_input == Some(input) && self.scaler.is_some() {
            return Ok(());
        }

        let src_pixel = pixel_for(format);
        self.scaler = Some(SwsContext::get(
            src_pixel,
            width,
            height,
            self.input_pixel,
            self.output_width,
            self.output_height,
            SwsFlags::FAST_BILINEAR,
        )?);
        self.src.set_format(src_pixel);
        self.src.set_width(width);
        self.src.set_height(height);
        self.last_input = Some(input);
        Ok(())
    }
}

fn open_named_encoder(
    name: &str,
    input_pixel: Pixel,
    output_width: u32,
    output_height: u32,
    fps: u32,
    bitrate: usize,
    options: Dictionary<'static>,
) -> Result<encoder::video::Encoder, ffmpeg::Error> {
    let codec = encoder::find_by_name(name).ok_or(ffmpeg::Error::EncoderNotFound)?;
    let mut context = codec::context::Context::new_with_codec(codec)
        .encoder()
        .video()?;
    context.set_width(output_width);
    context.set_height(output_height);
    context.set_format(input_pixel);
    context.set_time_base(Rational(1, VIDEO_TIME_BASE));
    context.set_frame_rate(Some(Rational(fps as i32, 1)));
    context.set_gop(fps);
    context.set_max_b_frames(0);
    context.set_bit_rate(bitrate);
    context.set_max_bit_rate(bitrate);
    context.set_flags(codec::Flags::LOW_DELAY | codec::Flags::CLOSED_GOP);
    // The GPU converter emits studio-range BT.709 NV12. These fields also
    // make the same output intent explicit when NVENC converts packed RGB.
    context.set_colorspace(Space::BT709);
    context.set_color_range(Range::MPEG);
    context.set_color_primaries(Primaries::BT709);
    context.set_color_transfer_characteristic(TransferCharacteristic::BT709);
    context.open_as_with(codec, options)
}

fn encoder_options(candidate: EncoderCandidate, input_pixel: Pixel) -> Dictionary<'static> {
    match candidate.backend {
        EncoderBackend::Av1Nvenc | EncoderBackend::HevcNvenc | EncoderBackend::H264Nvenc => {
            nvenc_options(candidate.codec, input_pixel)
        }
        EncoderBackend::Av1Amf | EncoderBackend::HevcAmf | EncoderBackend::H264Amf => {
            amf_options(candidate.codec)
        }
        EncoderBackend::Av1Qsv | EncoderBackend::HevcQsv | EncoderBackend::H264Qsv => {
            qsv_options(candidate.codec)
        }
        EncoderBackend::HevcVideoToolbox | EncoderBackend::H264VideoToolbox => {
            videotoolbox_options(candidate.codec)
        }
        EncoderBackend::None => Dictionary::new(),
    }
}

fn nvenc_options(codec: StreamCodec, input_pixel: Pixel) -> Dictionary<'static> {
    let mut options = Dictionary::new();
    // P4 is NVIDIA's balanced low-latency preset. The old P1 setting traded
    // away too much quality even when ample bitrate was available.
    options.set("preset", "p4");
    options.set("tune", "ull");
    options.set("rc", "cbr");
    options.set("delay", "0");
    options.set("zerolatency", "1");
    options.set("forced-idr", "1");
    options.set("rc-lookahead", "0");
    options.set("no-scenecut", "1");
    options.set("spatial-aq", "1");
    options.set("aq-strength", "8");
    options.set("multipass", "disabled");
    match codec {
        StreamCodec::H264 => {
            options.set("profile", "high");
            options.set("aud", "1");
        }
        StreamCodec::Hevc => {
            options.set("profile", "main");
            options.set("aud", "1");
        }
        StreamCodec::Av1 | StreamCodec::Auto => {}
    }
    if matches!(input_pixel, Pixel::RGBA | Pixel::BGRA) {
        options.set("rgb_mode", "yuv420");
    }
    options
}

fn amf_options(codec: StreamCodec) -> Dictionary<'static> {
    let mut options = Dictionary::new();
    options.set("usage", "lowlatency_high_quality");
    options.set("quality", "quality");
    options.set("rc", "cbr");
    options.set("async_depth", "1");
    options.set("forced_idr", "1");
    options.set("preencode", "0");
    match codec {
        StreamCodec::Av1 => {
            options.set("latency", "lowest_latency");
            options.set("header_insertion_mode", "idr");
        }
        StreamCodec::Hevc => {
            options.set("latency", "1");
            options.set("profile", "main");
            options.set("header_insertion_mode", "idr");
            options.set("aud", "1");
        }
        StreamCodec::H264 => {
            options.set("latency", "1");
            options.set("profile", "high");
            options.set("header_spacing", "1");
            options.set("aud", "1");
        }
        StreamCodec::Auto => {}
    }
    options
}

fn qsv_options(codec: StreamCodec) -> Dictionary<'static> {
    let mut options = Dictionary::new();
    options.set("async_depth", "1");
    options.set("preset", "medium");
    options.set("forced_idr", "1");
    options.set("low_delay_brc", "1");
    match codec {
        StreamCodec::H264 => {
            options.set("profile", "high");
            options.set("scenario", "remotegaming");
            options.set("aud", "1");
        }
        StreamCodec::Hevc => {
            options.set("profile", "main");
            options.set("scenario", "remotegaming");
            options.set("aud", "1");
        }
        StreamCodec::Av1 | StreamCodec::Auto => {}
    }
    options
}

fn videotoolbox_options(codec: StreamCodec) -> Dictionary<'static> {
    let mut options = Dictionary::new();
    // allow_sw=0 makes probing authoritative: success means VideoToolbox
    // obtained a hardware encoder, rather than silently using Apple's software
    // implementation.
    options.set("allow_sw", "0");
    options.set("realtime", "1");
    options.set("prio_speed", "0");
    // Some hardware sessions reject constant_bit_rate even when VideoToolbox
    // is available. Use the context's target bitrate (ABR) for portability.
    options.set("spatial_aq", "1");
    match codec {
        StreamCodec::H264 => options.set("profile", "high"),
        StreamCodec::Hevc => options.set("profile", "main"),
        StreamCodec::Auto | StreamCodec::Av1 => {}
    }
    options
}

// One ffmpeg packet is one encoded frame. The per-packet keyframe flag must
// stay attached to its own access unit: draining can return stale inter frames
// left over in the encoder alongside a freshly forced key frame, and batching
// them under a single OR-ed flag would feed clients (and the cached bootstrap
// key frame) an inter frame whose references they never received.
fn drain_packets(
    encoder: &mut encoder::video::Encoder,
    out: &mut Vec<(Vec<u8>, bool)>,
) -> Result<bool, ffmpeg::Error> {
    let mut packet = Packet::empty();
    let mut keyframe = false;
    loop {
        match encoder.receive_packet(&mut packet) {
            Ok(()) => {
                let is_key = packet.is_key();
                keyframe |= is_key;
                if let Some(data) = packet.data() {
                    if !data.is_empty() {
                        out.push((data.to_vec(), is_key));
                    }
                }
            }
            Err(ffmpeg::Error::Eof) => break,
            Err(ffmpeg::Error::Other { errno })
                if errno == ffmpeg::error::EAGAIN || errno == ffmpeg::error::EWOULDBLOCK =>
            {
                break;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(keyframe)
}

fn add_av1_temporal_delimiters(packets: &mut [(Vec<u8>, bool)]) {
    for (data, _) in packets.iter_mut() {
        if data.is_empty() {
            continue;
        }
        if !data.starts_with(&AV1_TEMPORAL_DELIMITER) {
            data.splice(0..0, AV1_TEMPORAL_DELIMITER.iter().copied());
        }
        // A trailing TD lets the raw-stream client emit this temporal unit
        // immediately instead of waiting for the next video frame.
        if !data.ends_with(&AV1_TEMPORAL_DELIMITER) {
            data.extend_from_slice(&AV1_TEMPORAL_DELIMITER);
        }
    }
}

pub(crate) fn pixel_for(format: PixelFormat) -> Pixel {
    match format {
        PixelFormat::Rgba8 => Pixel::RGBA,
        PixelFormat::Bgra8 => Pixel::BGRA,
        PixelFormat::Nv12 => Pixel::NV12,
    }
}

fn set_frame_timing(frame: &mut VideoFrame, pts: i64, force_keyframe: bool) {
    frame.set_pts(Some(pts));
    frame.set_kind(if force_keyframe {
        picture::Type::I
    } else {
        picture::Type::None
    });
    frame.set_color_primaries(Primaries::BT709);
    frame.set_color_transfer_characteristic(TransferCharacteristic::BT709);
    if matches!(frame.format(), Pixel::RGBA | Pixel::BGRA) {
        frame.set_color_space(Space::RGB);
        frame.set_color_range(Range::JPEG);
    } else {
        frame.set_color_space(Space::BT709);
        frame.set_color_range(Range::MPEG);
    }
}

fn telemetry_sei(sequence: u64, capture_unix_us: u64) -> Vec<u8> {
    const PAYLOAD_TYPE_USER_DATA_UNREGISTERED: u8 = 5;
    const PAYLOAD_SIZE: u8 = 32;

    let mut rbsp = Vec::with_capacity(35);
    rbsp.push(PAYLOAD_TYPE_USER_DATA_UNREGISTERED);
    rbsp.push(PAYLOAD_SIZE);
    rbsp.extend_from_slice(&TELEMETRY_UUID);
    rbsp.extend_from_slice(&sequence.to_be_bytes());
    rbsp.extend_from_slice(&capture_unix_us.to_be_bytes());
    rbsp.push(0x80);

    let mut nal = Vec::with_capacity(rbsp.len() + 8);
    nal.extend_from_slice(&[0, 0, 0, 1, 0x06]);
    let mut zero_count = 0;
    for byte in rbsp {
        if zero_count >= 2 && byte <= 3 {
            nal.push(3);
            zero_count = 0;
        }
        nal.push(byte);
        if byte == 0 {
            zero_count += 1;
        } else {
            zero_count = 0;
        }
    }
    nal
}

fn expected_frame_len(
    width: u32,
    height: u32,
    row_stride: u32,
    format: PixelFormat,
) -> Option<usize> {
    if width == 0 || height == 0 || row_stride == 0 || row_stride > i32::MAX as u32 {
        return None;
    }
    let stride = row_stride as usize;
    let height = height as usize;
    match format {
        PixelFormat::Rgba8 | PixelFormat::Bgra8 => {
            let minimum_stride = (width as usize).checked_mul(4)?;
            if stride < minimum_stride {
                return None;
            }
            stride.checked_mul(height)
        }
        PixelFormat::Nv12 => {
            if !width.is_multiple_of(2)
                || !height.is_multiple_of(2)
                || !row_stride.is_multiple_of(2)
                || row_stride < width
            {
                return None;
            }
            let luma = stride.checked_mul(height)?;
            let chroma = stride.checked_mul(height / 2)?;
            luma.checked_add(chroma)
        }
    }
}

fn validate_frame_layout(
    width: u32,
    height: u32,
    row_stride: u32,
    format: PixelFormat,
    len: usize,
) -> Result<(), ffmpeg::Error> {
    match expected_frame_len(width, height, row_stride, format) {
        Some(expected) if expected == len => Ok(()),
        _ => Err(ffmpeg::Error::InvalidData),
    }
}

pub(crate) fn attach_frame(
    dst: &mut VideoFrame,
    pixels: &[u8],
    width: u32,
    height: u32,
    row_stride: u32,
    format: PixelFormat,
) -> Result<(), ffmpeg::Error> {
    validate_frame_layout(width, height, row_stride, format, pixels.len())?;
    let stride = row_stride as usize;
    // The AVFrame only borrows CapturedFrame's Vec for the synchronous
    // sws_scale call. It has no AVBufferRef and is detached immediately after.
    unsafe {
        let raw = dst.as_mut_ptr();
        (*raw).data[0] = pixels.as_ptr().cast_mut();
        (*raw).linesize[0] = stride as i32;
        if format == PixelFormat::Nv12 {
            (*raw).data[1] = pixels.as_ptr().add(stride * height as usize).cast_mut();
            (*raw).linesize[1] = stride as i32;
        } else {
            (*raw).data[1] = std::ptr::null_mut();
            (*raw).linesize[1] = 0;
        }
    }
    Ok(())
}

pub(crate) fn detach_frame(dst: &mut VideoFrame) {
    unsafe {
        let raw = dst.as_mut_ptr();
        (*raw).data[0] = std::ptr::null_mut();
        (*raw).linesize[0] = 0;
        (*raw).data[1] = std::ptr::null_mut();
        (*raw).linesize[1] = 0;
    }
}

pub(crate) struct PooledPixels {
    pixels: Option<Vec<u8>>,
    pool: Arc<FrameBufferPool>,
}

impl PooledPixels {
    pub(crate) fn new(pixels: Vec<u8>, pool: Arc<FrameBufferPool>) -> Self {
        Self {
            pixels: Some(pixels),
            pool,
        }
    }

    pub(crate) fn as_slice(&self) -> &[u8] {
        self.pixels.as_deref().expect("pooled pixels")
    }

    fn len(&self) -> usize {
        self.as_slice().len()
    }
}

impl Drop for PooledPixels {
    fn drop(&mut self) {
        if let Some(pixels) = self.pixels.take() {
            self.pool.recycle(pixels);
        }
    }
}

fn owned_video_frame(
    pixels: PooledPixels,
    width: u32,
    height: u32,
    row_stride: u32,
    format: PixelFormat,
) -> Result<VideoFrame, ffmpeg::Error> {
    validate_frame_layout(width, height, row_stride, format, pixels.len())?;
    let mut pixels = Box::new(pixels);
    let data = pixels.pixels.as_mut().expect("pooled pixels").as_mut_ptr();
    let len = pixels.len();
    let opaque = Box::into_raw(pixels).cast::<c_void>();
    let buffer =
        unsafe { ffmpeg::ffi::av_buffer_create(data, len, Some(free_pooled_pixels), opaque, 0) };
    if buffer.is_null() {
        // av_buffer_create leaves the supplied data untouched on failure.
        unsafe {
            drop(Box::from_raw(opaque.cast::<PooledPixels>()));
        }
        return Err(ffmpeg::Error::Other {
            errno: ffmpeg::error::ENOMEM,
        });
    }

    let mut frame = VideoFrame::empty();
    frame.set_format(pixel_for(format));
    frame.set_width(width);
    frame.set_height(height);
    unsafe {
        let raw = frame.as_mut_ptr();
        (*raw).buf[0] = buffer;
        (*raw).data[0] = data;
        (*raw).linesize[0] = row_stride as i32;
        if format == PixelFormat::Nv12 {
            (*raw).data[1] = data.add(row_stride as usize * height as usize);
            (*raw).linesize[1] = row_stride as i32;
        }
    }
    Ok(frame)
}

unsafe extern "C" fn free_pooled_pixels(opaque: *mut c_void, _data: *mut u8) {
    if opaque.is_null() {
        return;
    }
    // A poisoned pool mutex must never unwind through FFmpeg's C callback.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        drop(Box::from_raw(opaque.cast::<PooledPixels>()));
    }));
}

pub fn run_encoder_thread(shared: Arc<SharedState>) {
    let mut encoder: Option<VideoEncoder> = None;
    let mut packets: Vec<(Vec<u8>, bool)> = Vec::new();
    let mut last_log = Instant::now();
    let mut last_submitted = 0;
    let mut last_encoded = 0;
    let mut last_readback_bytes = 0;
    while shared.running.load(Ordering::SeqCst) {
        let Some(frame) = shared.mailbox.wait_take(&shared.running) else {
            break;
        };
        let has_clients = shared.client_count.load(Ordering::Relaxed) > 0;
        let want_keyframe = shared.force_keyframe.load(Ordering::SeqCst);
        if !has_clients && !want_keyframe {
            shared.frame_pool.recycle(frame.pixels);
            continue;
        }

        let output_dimensions = (frame.output_width, frame.output_height);
        let preferred_input = frame.format;
        let must_reconfigure = encoder
            .as_ref()
            .is_none_or(|encoder| encoder.needs_reconfigure(output_dimensions, preferred_input));
        if must_reconfigure {
            // Once a wire codec has been selected, a resize must reopen that
            // exact encoder. Re-probing could otherwise change codec while a
            // raw-stream client is connected.
            let replacement_result = match encoder.as_ref().map(|current| current.selection) {
                Some(selection) => VideoEncoder::open_candidate(
                    shared.config,
                    selection,
                    output_dimensions.0,
                    output_dimensions.1,
                    preferred_input,
                ),
                None => VideoEncoder::new(
                    shared.config,
                    output_dimensions.0,
                    output_dimensions.1,
                    preferred_input,
                ),
            };
            let replacement = match replacement_result {
                Ok(encoder) => encoder,
                Err(error) => {
                    shared.frame_pool.recycle(frame.pixels);
                    godot::prelude::godot_error!(
                        "game_stream: {} encoder init failed for {}x{}@{}: {error}",
                        shared.config.codec.as_str(),
                        output_dimensions.0,
                        output_dimensions.1,
                        shared.config.fps,
                    );
                    shared.request_stop();
                    return;
                }
            };
            let backend = replacement.backend();
            shared
                .encoder_backend
                .store(backend as u32, Ordering::SeqCst);
            *shared.active_codec.lock().expect("active codec mutex") = replacement.codec();
            shared.force_keyframe.store(true, Ordering::SeqCst);
            godot::prelude::godot_print!(
                "game_stream: codec={} encoder={} input={} output={}x{}@{} bitrate={:.2}Mbps",
                replacement.codec().as_str(),
                backend.as_str(),
                preferred_input.as_str(),
                output_dimensions.0,
                output_dimensions.1,
                shared.config.fps,
                shared
                    .config
                    .bitrate_for(output_dimensions.0, output_dimensions.1) as f64
                    / 1_000_000.0,
            );
            encoder = Some(replacement);
        }

        let force_keyframe = shared.force_keyframe.swap(false, Ordering::SeqCst);
        let sequence = frame.sequence;
        let timeline = frame.timeline;
        let encode_started_at = Instant::now();
        shared.encode_queue_us.store(
            encode_started_at
                .saturating_duration_since(timeline.ready_for_encode_at)
                .as_micros() as u64,
            Ordering::Relaxed,
        );
        packets.clear();
        let (timing, produced_keyframe) =
            match encoder.as_mut().expect("configured encoder").encode(
                frame,
                Arc::clone(&shared.frame_pool),
                force_keyframe,
                &mut packets,
            ) {
                Ok(result) => result,
                Err(error) => {
                    godot::prelude::godot_warn!("game_stream: encode failed: {error}");
                    shared.force_keyframe.store(true, Ordering::SeqCst);
                    continue;
                }
            };
        let encoded_at = Instant::now();

        shared.scale_us.store(timing.scale_us, Ordering::Relaxed);
        shared.encode_us.store(timing.encode_us, Ordering::Relaxed);
        shared.encoder_stage_us.store(
            encoded_at
                .saturating_duration_since(encode_started_at)
                .as_micros() as u64,
            Ordering::Relaxed,
        );
        shared.capture_to_encoded_us.store(
            encoded_at
                .saturating_duration_since(timeline.post_draw_at)
                .as_micros() as u64,
            Ordering::Relaxed,
        );
        shared.encode_ok.fetch_add(1, Ordering::Relaxed);
        if timing.direct_input {
            shared.encoder_direct_frames.fetch_add(1, Ordering::Relaxed);
        }

        if force_keyframe && !produced_keyframe {
            // Don't lose the request if NVENC/x264 skipped this frame or
            // ignored pict_type; a waiting client still needs a key frame.
            shared.force_keyframe.store(true, Ordering::SeqCst);
        }
        if packets.is_empty() {
            shared.encoder_no_output.fetch_add(1, Ordering::Relaxed);
        } else {
            let active_codec = encoder.as_ref().expect("configured encoder").codec();
            // HEVC uses a different SEI NAL header and AV1 uses metadata OBUs.
            // The telemetry SEI rides with the last packet, which is the frame
            // produced from the capture currently being encoded.
            let total = packets.len();
            let mut dropped_any = false;
            for (index, (data, is_key)) in packets.drain(..).enumerate() {
                let mut access_unit = Vec::with_capacity(2);
                if active_codec == StreamCodec::H264 && index + 1 == total {
                    access_unit.push(telemetry_sei(sequence, timeline.capture_unix_us));
                }
                access_unit.push(data);
                let encoded = EncodedFrame {
                    packets: access_unit,
                    codec: active_codec,
                    keyframe: is_key,
                    sequence,
                    timeline,
                    encode_started_at,
                    encoded_at,
                    scale_us: timing.scale_us,
                    encode_us: timing.encode_us,
                };
                if shared.encoded_mailbox.push(encoded) {
                    dropped_any = true;
                }
            }
            if dropped_any {
                shared.dropped_after_encode.fetch_add(1, Ordering::Relaxed);
                shared.force_keyframe.store(true, Ordering::SeqCst);
            }
        }

        if last_log.elapsed().as_secs() >= 5 {
            let elapsed = last_log.elapsed().as_secs_f64();
            let submitted = shared.submitted.load(Ordering::Relaxed);
            let encoded = shared.encode_ok.load(Ordering::Relaxed);
            let readback_bytes = shared.readback_bytes.load(Ordering::Relaxed);
            let submit_fps = (submitted - last_submitted) as f64 / elapsed;
            let encode_fps = (encoded - last_encoded) as f64 / elapsed;
            let readback_mib_s =
                (readback_bytes - last_readback_bytes) as f64 / elapsed / (1024.0 * 1024.0);
            last_log = Instant::now();
            last_submitted = submitted;
            last_encoded = encoded;
            last_readback_bytes = readback_bytes;
            let timing = shared.timing_summary();
            let backend = shared.encoder_backend();
            let readback_format =
                PixelFormat::from_u32(shared.last_readback_format.load(Ordering::Relaxed))
                    .map_or("none", PixelFormat::as_str);
            godot::prelude::godot_print!(
                "game_stream: codec={} encoder={} output={}x{}@{} capture={:.1}fps encode={:.1}fps delivered={:.1}fps bitrate={:.2}Mbps raw={:.1}MiB/s in_flight={} drop_raw={} drop_net={} encoder_direct={} gpu_nv12={} source={}x{} readback={}x{}:{}/{:.2}MiB timing_samples={} seq={}",
                encoder
                    .as_ref()
                    .expect("configured encoder")
                    .codec()
                    .as_str(),
                backend.as_str(),
                shared.output_width.load(Ordering::Relaxed),
                shared.output_height.load(Ordering::Relaxed),
                shared.config.fps,
                submit_fps,
                encode_fps,
                timing.delivered_fps,
                timing.stream_bitrate_bps as f64 / 1_000_000.0,
                readback_mib_s,
                shared.in_flight.load(Ordering::Relaxed),
                shared.dropped_before_encode.load(Ordering::Relaxed),
                shared.dropped_after_encode.load(Ordering::Relaxed),
                shared.encoder_direct_frames.load(Ordering::Relaxed),
                shared.gpu_nv12_frames.load(Ordering::Relaxed),
                shared.last_width.load(Ordering::Relaxed),
                shared.last_height.load(Ordering::Relaxed),
                shared.last_readback_width.load(Ordering::Relaxed),
                shared.last_readback_height.load(Ordering::Relaxed),
                readback_format,
                shared.last_readback_bytes.load(Ordering::Relaxed) as f64
                    / (1024.0 * 1024.0),
                timing.samples,
                sequence,
            );
            godot::prelude::godot_print!(
                "game_stream timing_us p50/p95: post_to_rd={}/{} rd_setup={}/{} readback={}/{} copy={}/{} encode_queue={}/{} scale={}/{} codec={}/{} encoder_stage={}/{} network_queue={}/{} packet_prep={}/{} socket={}/{} total={}/{}",
                timing.post_to_render.p50_us,
                timing.post_to_render.p95_us,
                timing.render_setup.p50_us,
                timing.render_setup.p95_us,
                timing.readback_wait.p50_us,
                timing.readback_wait.p95_us,
                timing.callback_copy.p50_us,
                timing.callback_copy.p95_us,
                timing.encode_queue.p50_us,
                timing.encode_queue.p95_us,
                timing.scale.p50_us,
                timing.scale.p95_us,
                timing.encode.p50_us,
                timing.encode.p95_us,
                timing.encoder_stage.p50_us,
                timing.encoder_stage.p95_us,
                timing.network_queue.p50_us,
                timing.network_queue.p95_us,
                timing.packet_prep.p50_us,
                timing.packet_prep.p95_us,
                timing.socket_write.p50_us,
                timing.socket_write.p95_us,
                timing.total.p50_us,
                timing.total.p95_us,
            );
        }
    }
}

pub fn init_ffmpeg() -> Result<(), ffmpeg::Error> {
    // Reject accidental GPL/nonfree SDKs in the LGPL distribution.
    let license = unsafe { std::ffi::CStr::from_ptr(ffmpeg::ffi::avcodec_license()) };
    if !license.to_bytes().starts_with(b"LGPL") {
        return Err(ffmpeg::Error::InvalidData);
    }
    ffmpeg::init()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_sei_is_one_escaped_annex_b_nal() {
        let packet = telemetry_sei(0x0000_0001, 0x0000_0002);
        assert!(packet.starts_with(&[0, 0, 0, 1, 0x06, 5, 32]));
        assert_eq!(
            packet[4..]
                .windows(3)
                .filter(|window| *window == [0, 0, 1])
                .count(),
            0,
            "RBSP must escape embedded Annex-B start codes"
        );
    }

    #[test]
    fn candidates_are_ordered_by_wire_codec() {
        let codecs = ENCODER_CANDIDATES
            .iter()
            .map(|candidate| candidate.codec)
            .collect::<Vec<_>>();
        let av1_end = codecs
            .iter()
            .rposition(|codec| *codec == StreamCodec::Av1)
            .unwrap_or(0);
        let hevc_start = codecs
            .iter()
            .position(|codec| *codec == StreamCodec::Hevc)
            .expect("HEVC candidate");
        let h264_start = codecs
            .iter()
            .position(|codec| *codec == StreamCodec::H264)
            .expect("H.264 candidate");
        assert!(av1_end < hevc_start || !codecs.contains(&StreamCodec::Av1));
        assert!(hevc_start < h264_start);
        assert_eq!(codecs.last(), Some(&StreamCodec::H264));
    }

    #[test]
    fn av1_access_unit_gets_leading_and_trailing_delimiters() {
        let mut packets = vec![
            (vec![0x0a, 0x01, 0x00], false),
            (vec![0x0a, 0x02, 0x00], false),
        ];
        add_av1_temporal_delimiters(&mut packets);
        for (data, _) in &packets {
            assert!(data.starts_with(&AV1_TEMPORAL_DELIMITER));
            assert!(data.ends_with(&AV1_TEMPORAL_DELIMITER));
        }

        // Normalization must be idempotent: a second pass changes no bytes.
        let flattened: Vec<u8> = packets
            .iter()
            .flat_map(|(data, _)| data.iter().copied())
            .collect();
        add_av1_temporal_delimiters(&mut packets);
        let reflattened: Vec<u8> = packets
            .iter()
            .flat_map(|(data, _)| data.iter().copied())
            .collect();
        assert_eq!(
            flattened, reflattened,
            "normalization must not stack delimiters"
        );
    }

    #[test]
    fn owned_avframe_recycles_pixels_after_the_last_reference() {
        let pool = Arc::new(FrameBufferPool::new());
        let config = StreamConfig::default();
        let width = config.requested_width;
        let height = config.requested_height;
        let len = width as usize * height as usize * 4;
        let pixels = PooledPixels::new(vec![17; len], Arc::clone(&pool));
        let frame = owned_video_frame(pixels, width, height, width * 4, PixelFormat::Rgba8)
            .expect("owned AVFrame");
        assert_eq!(frame.data(0)[0], 17);

        let mut retained = VideoFrame::empty();
        let result = unsafe { ffmpeg::ffi::av_frame_ref(retained.as_mut_ptr(), frame.as_ptr()) };
        assert_eq!(result, 0);
        drop(frame);
        assert_eq!(pool.available(), 0, "encoder reference must retain pixels");
        drop(retained);
        assert_eq!(pool.available(), 1, "last AVFrame must recycle pixels");
    }

    #[test]
    fn nv12_avframe_points_at_both_planes_in_one_owned_buffer() {
        let pool = Arc::new(FrameBufferPool::new());
        let width = 6;
        let height = 4;
        let stride = 8;
        let len = stride as usize * height as usize * 3 / 2;
        let pixels = PooledPixels::new((0..len as u8).collect(), Arc::clone(&pool));
        let mut frame = owned_video_frame(pixels, width, height, stride, PixelFormat::Nv12)
            .expect("owned NV12 AVFrame");

        unsafe {
            let raw = frame.as_ptr();
            assert_eq!(
                (*raw).format,
                ffmpeg::ffi::AVPixelFormat::AV_PIX_FMT_NV12 as i32
            );
            assert_eq!((*raw).linesize[0], 8);
            assert_eq!((*raw).linesize[1], 8);
            assert_eq!((*raw).data[1].offset_from((*raw).data[0]), 32);
            assert_eq!(*(*raw).data[1], 32);
        }
        set_frame_timing(&mut frame, 90_000, false);
        assert_eq!(frame.color_space(), Space::BT709);
        assert_eq!(frame.color_range(), Range::MPEG);

        drop(frame);
        assert_eq!(pool.available(), 1);
    }

    #[test]
    fn frame_layout_validation_covers_packed_and_nv12_stride() {
        assert_eq!(expected_frame_len(4, 2, 16, PixelFormat::Rgba8), Some(32));
        assert_eq!(expected_frame_len(6, 4, 8, PixelFormat::Nv12), Some(48));
        assert_eq!(expected_frame_len(5, 4, 8, PixelFormat::Nv12), None);
        assert!(validate_frame_layout(6, 4, 8, PixelFormat::Nv12, 47).is_err());
    }
}
