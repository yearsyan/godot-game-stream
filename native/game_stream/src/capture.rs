// SPDX-License-Identifier: LGPL-2.1-or-later
// Copyright (C) 2026 yearsyan and contributors

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use godot::builtin::{Array, Callable, PackedByteArray, Rid};
use godot::classes::rendering_device::{
    DataFormat, SamplerFilter, SamplerRepeatMode, ShaderStage, TextureSamples, TextureType,
    TextureUsageBits, UniformType,
};
use godot::classes::{
    Engine, RdSamplerState, RdShaderSource, RdTextureFormat, RdTextureView, RdUniform,
    RenderingDevice, RenderingServer, SceneTree,
};
use godot::global::Error;
use godot::obj::NewGd;
use godot::prelude::*;

use crate::shared::{
    CaptureTimeline, CapturedFrame, DownscaleResources, DownscaleState, GpuReadbackKind,
    PixelFormat, SharedState, DOWNSCALE_BUFFER_COUNT, VIDEO_TIME_BASE,
};

const DOWNSCALE_WORKGROUP_SIZE: u32 = 8;
const NV12_PIXELS_PER_INVOCATION_X: u32 = 4;
const NV12_PIXELS_PER_INVOCATION_Y: u32 = 2;
const RGBA_DOWNSCALE_SHADER: &str = r#"#version 450
layout(local_size_x = 8, local_size_y = 8, local_size_z = 1) in;

layout(set = 0, binding = 0) uniform sampler2D source_texture;
layout(rgba8, set = 0, binding = 1) uniform writeonly image2D output_texture;

void main() {
    ivec2 output_position = ivec2(gl_GlobalInvocationID.xy);
    ivec2 output_size = imageSize(output_texture);
    if (any(greaterThanEqual(output_position, output_size))) {
        return;
    }

    vec2 uv = (vec2(output_position) + vec2(0.5)) / vec2(output_size);
    imageStore(output_texture, output_position, textureLod(source_texture, uv, 0.0));
}
"#;

const NV12_PREPROCESS_SHADER: &str = r#"#version 450
layout(local_size_x = 8, local_size_y = 8, local_size_z = 1) in;

layout(set = 0, binding = 0) uniform sampler2D source_texture;
layout(set = 0, binding = 1, std430) writeonly buffer Nv12Output {
    uint words[];
} output_buffer;

layout(push_constant, std430) uniform Params {
    uvec2 output_size;
    uint stride_words;
    uint uv_offset_words;
} params;

vec3 sample_output_pixel(uint x, uint y) {
    uvec2 clamped = min(uvec2(x, y), params.output_size - uvec2(1));
    vec2 uv = (vec2(clamped) + vec2(0.5)) / vec2(params.output_size);
    return textureLod(source_texture, uv, 0.0).rgb;
}

uint byte_value(float value) {
    return uint(clamp(floor(value * 255.0 + 0.5), 0.0, 255.0));
}

uint luma_bt709_limited(vec3 rgb) {
    return byte_value(0.062745098 + dot(rgb, vec3(0.182586, 0.614231, 0.062007)));
}

uvec2 chroma_bt709_limited(vec3 rgb) {
    float u = 0.501960784 + dot(rgb, vec3(-0.100644, -0.338572, 0.439216));
    float v = 0.501960784 + dot(rgb, vec3(0.439216, -0.398942, -0.040274));
    return uvec2(byte_value(u), byte_value(v));
}

uint pack_bytes(uint a, uint b, uint c, uint d) {
    return a | (b << 8) | (c << 16) | (d << 24);
}

void main() {
    uint x = gl_GlobalInvocationID.x * 4;
    uint y = gl_GlobalInvocationID.y * 2;
    if (x >= params.output_size.x || y >= params.output_size.y) {
        return;
    }

    vec3 top0 = sample_output_pixel(x, y);
    vec3 top1 = sample_output_pixel(x + 1, y);
    vec3 top2 = sample_output_pixel(x + 2, y);
    vec3 top3 = sample_output_pixel(x + 3, y);
    vec3 bottom0 = sample_output_pixel(x, y + 1);
    vec3 bottom1 = sample_output_pixel(x + 1, y + 1);
    vec3 bottom2 = sample_output_pixel(x + 2, y + 1);
    vec3 bottom3 = sample_output_pixel(x + 3, y + 1);

    uint word_x = x >> 2;
    output_buffer.words[y * params.stride_words + word_x] = pack_bytes(
        luma_bt709_limited(top0), luma_bt709_limited(top1),
        luma_bt709_limited(top2), luma_bt709_limited(top3));
    output_buffer.words[(y + 1) * params.stride_words + word_x] = pack_bytes(
        luma_bt709_limited(bottom0), luma_bt709_limited(bottom1),
        luma_bt709_limited(bottom2), luma_bt709_limited(bottom3));

    uvec2 uv0 = chroma_bt709_limited((top0 + top1 + bottom0 + bottom1) * 0.25);
    uvec2 uv1 = chroma_bt709_limited((top2 + top3 + bottom2 + bottom3) * 0.25);
    output_buffer.words[params.uv_offset_words + (y >> 1) * params.stride_words + word_x] =
        pack_bytes(uv0.x, uv0.y, uv1.x, uv1.y);
}
"#;

#[derive(Clone, Copy)]
enum AsyncReadback {
    Texture(Rid),
    Buffer(Rid),
}

#[derive(Clone, Copy)]
struct CaptureMetadata {
    generation: u64,
    sequence: u64,
    capture_unix_us: u64,
    post_draw_at: Instant,
    render_thread_at: Instant,
    readback_submitted_at: Instant,
    width: u32,
    height: u32,
    row_stride: u32,
    output_width: u32,
    output_height: u32,
    format: PixelFormat,
    pts: i64,
    downscale_slot: Option<usize>,
}

/// Minimum quiet time before the watchdog forces a redraw. Long enough that
/// an actively rendering game never gets double-drawn, short enough that a
/// client connecting to an idle scene sees a keyframe quickly.
const FORCED_DRAW_MIN_INTERVAL_US: u64 = 100_000;

pub fn connect_post_draw(shared: &Arc<SharedState>) -> Callable {
    let state = Arc::clone(shared);
    let callable = Callable::from_sync_fn("game_stream_post_draw", move |_args| {
        state
            .last_post_draw_us
            .store(unix_time_us(), Ordering::Relaxed);
        // frame_post_draw is deferred onto the main thread when the renderer
        // is threaded, which is where Input.parse_input_event must run.
        crate::input::inject_pending(&state);
        submit_frame(&state);
    });
    let mut rs = RenderingServer::singleton();
    let error = rs.connect("frame_post_draw", &callable);
    if error != Error::OK {
        godot_error!("game_stream: failed to connect frame_post_draw: {error:?}");
    }
    callable
}

pub fn disconnect_post_draw(callable: &Callable) {
    let mut rs = RenderingServer::singleton();
    rs.disconnect("frame_post_draw", callable);
}

/// Godot only emits frame_post_draw for frames it actually draws, so when the
/// game stops redrawing (fully static scene) a pending keyframe request can
/// never be satisfied. This watchdog runs on SceneTree.process_frame (emitted
/// every process iteration, draw or not) and forces one redraw while a
/// keyframe is pending and the game has gone quiet, so connect-time IDR
/// requests always complete. `force_draw` must run on the main thread, which
/// process_frame guarantees.
pub fn connect_draw_watchdog(shared: &Arc<SharedState>) -> Option<Callable> {
    let Some(mut tree) = current_scene_tree() else {
        godot_error!("game_stream: no SceneTree; forced-redraw watchdog disabled");
        return None;
    };
    let state = Arc::clone(shared);
    let callable = Callable::from_sync_fn("game_stream_draw_watchdog", move |_args| {
        if !state.running.load(Ordering::SeqCst) {
            return;
        }
        let now_us = unix_time_us();
        if !should_force_draw(
            state.force_keyframe.load(Ordering::SeqCst),
            state.last_post_draw_us.load(Ordering::Relaxed),
            state.last_forced_draw_us.load(Ordering::Relaxed),
            now_us,
        ) {
            return;
        }
        state.last_forced_draw_us.store(now_us, Ordering::Relaxed);
        RenderingServer::singleton().force_draw();
    });
    let error = tree.connect("process_frame", &callable);
    if error != Error::OK {
        godot_error!("game_stream: failed to connect process_frame watchdog: {error:?}");
        return None;
    }
    Some(callable)
}

pub fn disconnect_draw_watchdog(callable: &Callable) {
    let Some(mut tree) = current_scene_tree() else {
        return;
    };
    tree.disconnect("process_frame", callable);
}

fn should_force_draw(
    keyframe_pending: bool,
    last_post_draw_us: u64,
    last_forced_draw_us: u64,
    now_us: u64,
) -> bool {
    if !keyframe_pending {
        return false;
    }
    // 0 means "no draw since streaming started"; saturating_sub makes that
    // look infinitely stale, which is what the startup bootstrap wants.
    now_us.saturating_sub(last_post_draw_us.max(last_forced_draw_us)) >= FORCED_DRAW_MIN_INTERVAL_US
}

fn current_scene_tree() -> Option<Gd<SceneTree>> {
    Engine::singleton()
        .get_main_loop()?
        .try_cast::<SceneTree>()
        .ok()
}

fn unix_time_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros()
        .min(u64::MAX as u128) as u64
}

pub fn request_gpu_cleanup(state: &Arc<SharedState>) {
    state.cleanup_requested.store(true, Ordering::SeqCst);
    let cleanup_state = Arc::clone(state);
    let callable = Callable::from_sync_fn("game_stream_rd_cleanup", move |_args| {
        cleanup_if_idle(&cleanup_state);
    });
    RenderingServer::singleton().call_on_render_thread(&callable);
}

fn submit_frame(state: &Arc<SharedState>) {
    let post_draw_at = Instant::now();
    let capture_unix_us = unix_time_us();
    if !state.running.load(Ordering::SeqCst) {
        return;
    }
    if state.client_count.load(Ordering::Relaxed) == 0 {
        // Still capture while an IDR is requested so the net thread can cache
        // a bootstrap keyframe. Otherwise a client that connects after the
        // game stops redrawing waits forever for the first IDR.
        if !state.force_keyframe.load(Ordering::SeqCst) {
            state.dropped_no_client.fetch_add(1, Ordering::Relaxed);
            return;
        }
    }

    let min_interval = Duration::from_nanos(1_000_000_000 / u64::from(state.config.fps));
    {
        let now = std::time::Instant::now();
        let mut clock = state.submit_clock.lock().expect("submit throttle mutex");
        if !clock.should_submit(now, min_interval) {
            state.throttled.fetch_add(1, Ordering::Relaxed);
            return;
        }
    }

    loop {
        let in_flight = state.in_flight.load(Ordering::SeqCst);
        if in_flight >= crate::shared::MAX_IN_FLIGHT {
            state.dropped_inflight.fetch_add(1, Ordering::Relaxed);
            recover_stuck_in_flight(state);
            return;
        }
        if state
            .in_flight
            .compare_exchange(in_flight, in_flight + 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            break;
        }
    }

    // frame_post_draw is deferred onto the main thread when the renderer is
    // threaded. texture_get_data_async must run on the RD/render thread.
    let state_rt = Arc::clone(state);
    let callable = Callable::from_sync_fn("game_stream_rd_request", move |_args| {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            request_texture(&state_rt, post_draw_at, capture_unix_us)
        }));
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                finish_in_flight(&state_rt);
                godot_error!("game_stream: capture request failed: {error:?}");
            }
            Err(_) => {
                finish_in_flight(&state_rt);
                godot_error!("game_stream: capture request panicked");
            }
        }
    });
    RenderingServer::singleton().call_on_render_thread(&callable);
}

fn request_texture(
    state: &Arc<SharedState>,
    post_draw_at: Instant,
    capture_unix_us: u64,
) -> Result<(), Error> {
    let render_thread_at = Instant::now();
    let rs = RenderingServer::singleton();
    let Some(mut rd) = rs.get_rendering_device() else {
        return Err(Error::ERR_UNAVAILABLE);
    };

    let viewport = *state.viewport.lock().expect("viewport mutex");
    if !viewport.is_valid() {
        return Err(Error::ERR_UNAVAILABLE);
    }

    let texture = rs.viewport_get_texture(viewport);
    if !texture.is_valid() {
        return Err(Error::ERR_UNAVAILABLE);
    }

    let rd_texture = rs.texture_get_rd_texture(texture);
    if !rd_texture.is_valid() {
        return Err(Error::ERR_UNAVAILABLE);
    }

    let Some(format_info) = rd.texture_get_format(rd_texture) else {
        return Err(Error::ERR_UNAVAILABLE);
    };
    let source_width = format_info.get_width();
    let source_height = format_info.get_height();
    let rd_format = format_info.get_format();
    let Some(source_pixel_format) = map_rd_format(rd_format) else {
        log_unsupported_format(rd_format);
        return Err(Error::ERR_UNAVAILABLE);
    };

    if source_width == 0 || source_height == 0 {
        return Err(Error::ERR_UNAVAILABLE);
    }
    let Some((output_width, output_height)) = state
        .config
        .dimensions_for_source(source_width, source_height)
    else {
        return Err(Error::ERR_INVALID_PARAMETER);
    };

    state.last_width.store(source_width, Ordering::Relaxed);
    state.last_height.store(source_height, Ordering::Relaxed);
    let previous_output_width = state.output_width.swap(output_width, Ordering::Relaxed);
    let previous_output_height = state.output_height.swap(output_height, Ordering::Relaxed);
    let target_bitrate = state.config.bitrate_for(output_width, output_height) as u64;
    state
        .target_bitrate
        .store(target_bitrate, Ordering::Relaxed);
    if previous_output_width != output_width || previous_output_height != output_height {
        godot_print!(
            "game_stream: source={}x{} requested={}x{} effective={}x{}@{} bitrate={:.2}Mbps",
            source_width,
            source_height,
            state.config.requested_width,
            state.config.requested_height,
            output_width,
            output_height,
            state.config.fps,
            target_bitrate as f64 / 1_000_000.0,
        );
    }

    let source_row_stride = source_width
        .checked_mul(4)
        .ok_or(Error::ERR_OUT_OF_MEMORY)?;
    let (readback, width, height, row_stride, pixel_format, downscale_slot) =
        if should_gpu_preprocess(rd_format) {
            match dispatch_gpu_preprocess(&mut rd, state, rd_texture, output_width, output_height) {
                DownscaleDispatch::Ready(output) => (
                    output.readback,
                    output_width,
                    output_height,
                    output.row_stride,
                    output.format,
                    Some(output.slot),
                ),
                DownscaleDispatch::Disabled => {
                    state
                        .gpu_downscale_fallbacks
                        .fetch_add(1, Ordering::Relaxed);
                    (
                        AsyncReadback::Texture(rd_texture),
                        source_width,
                        source_height,
                        source_row_stride,
                        source_pixel_format,
                        None,
                    )
                }
                DownscaleDispatch::Busy => return Err(Error::ERR_BUSY),
            }
        } else {
            state.gpu_downscale_active.store(false, Ordering::Relaxed);
            (
                AsyncReadback::Texture(rd_texture),
                source_width,
                source_height,
                source_row_stride,
                source_pixel_format,
                None,
            )
        };

    state.last_readback_width.store(width, Ordering::Relaxed);
    state.last_readback_height.store(height, Ordering::Relaxed);
    state
        .last_readback_format
        .store(pixel_format as u32, Ordering::Relaxed);

    let generation = state.generation.load(Ordering::SeqCst);
    let sequence = state.sequence.fetch_add(1, Ordering::Relaxed);
    let readback_submitted_at = Instant::now();
    let pts = duration_to_pts(post_draw_at.saturating_duration_since(state.started));
    let metadata = CaptureMetadata {
        generation,
        sequence,
        capture_unix_us,
        post_draw_at,
        render_thread_at,
        readback_submitted_at,
        width,
        height,
        row_stride,
        output_width,
        output_height,
        format: pixel_format,
        pts,
        downscale_slot,
    };
    let callback_state = Arc::clone(state);
    let callback = Callable::from_sync_fn("game_stream_readback", move |args| {
        if let Err(_panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            on_readback_data(&callback_state, metadata, args);
        })) {
            godot_error!("game_stream: readback callback panicked");
        }
    });

    let error = match readback {
        AsyncReadback::Texture(texture) => rd.texture_get_data_async(texture, 0, &callback),
        AsyncReadback::Buffer(buffer) => rd.buffer_get_data_async(buffer, &callback),
    };
    if error != Error::OK {
        release_downscale_slot(state, downscale_slot);
        return Err(error);
    }

    state.post_to_render_us.store(
        render_thread_at
            .saturating_duration_since(post_draw_at)
            .as_micros() as u64,
        Ordering::Relaxed,
    );
    state.render_setup_us.store(
        readback_submitted_at
            .saturating_duration_since(render_thread_at)
            .as_micros() as u64,
        Ordering::Relaxed,
    );
    state.submitted.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

fn on_readback_data(state: &Arc<SharedState>, metadata: CaptureMetadata, args: &[&Variant]) {
    let callback_at = Instant::now();
    let CaptureMetadata {
        generation,
        sequence,
        capture_unix_us,
        post_draw_at,
        render_thread_at,
        readback_submitted_at,
        width,
        height,
        row_stride,
        output_width,
        output_height,
        format,
        pts,
        downscale_slot,
    } = metadata;
    release_downscale_slot(state, downscale_slot);
    finish_in_flight(state);
    state.completed.fetch_add(1, Ordering::Relaxed);
    state.callback_delay_us.store(
        callback_at
            .saturating_duration_since(readback_submitted_at)
            .as_micros() as u64,
        Ordering::Relaxed,
    );

    if generation != state.generation.load(Ordering::SeqCst) {
        return;
    }
    if !state.running.load(Ordering::SeqCst) {
        return;
    }
    // Mirror the submit_frame gate: with no client but a pending keyframe
    // request the frame must still reach the encoder, otherwise the net
    // thread's cached IDR can never be (re)populated while idle.
    if state.client_count.load(Ordering::Relaxed) == 0
        && !state.force_keyframe.load(Ordering::SeqCst)
    {
        return;
    }

    let Some(variant) = args.first() else {
        return;
    };
    let Ok(bytes) = variant.try_to::<PackedByteArray>() else {
        return;
    };

    let slice = bytes.as_slice();
    if slice.is_empty() {
        return;
    }
    let Some(row_stride) = validate_readback_layout(format, width, height, row_stride, slice.len())
    else {
        godot_warn!(
            "game_stream: unexpected {} readback layout: {} bytes, {}x{}, declared stride {}",
            format.as_str(),
            slice.len(),
            width,
            height,
            row_stride,
        );
        return;
    };
    state
        .last_readback_bytes
        .store(slice.len() as u64, Ordering::Relaxed);
    state
        .readback_bytes
        .fetch_add(slice.len() as u64, Ordering::Relaxed);

    let copy_started = Instant::now();
    let mut pixels = state.frame_pool.take(slice.len());
    pixels.copy_from_slice(slice);
    let ready_for_encode_at = Instant::now();
    state.callback_copy_us.store(
        ready_for_encode_at
            .saturating_duration_since(copy_started)
            .as_micros() as u64,
        Ordering::Relaxed,
    );

    let frame = CapturedFrame {
        pixels,
        width,
        height,
        row_stride,
        output_width,
        output_height,
        format,
        sequence,
        timeline: CaptureTimeline {
            capture_unix_us,
            post_draw_at,
            render_thread_at,
            readback_submitted_at,
            callback_at,
            ready_for_encode_at,
        },
        pts,
    };
    if let Some(dropped) = state.mailbox.push(frame) {
        state.frame_pool.recycle(dropped.pixels);
        state.dropped_before_encode.fetch_add(1, Ordering::Relaxed);
    }
}

fn should_gpu_preprocess(format: DataFormat) -> bool {
    matches!(
        format,
        DataFormat::R8G8B8A8_UNORM
            | DataFormat::R8G8B8A8_SRGB
            | DataFormat::B8G8R8A8_UNORM
            | DataFormat::B8G8R8A8_SRGB
    )
}

fn nv12_layout(width: u32, height: u32) -> Option<(u32, u32, u32)> {
    if width < 2 || height < 2 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return None;
    }
    let row_stride = width.checked_add(3)? & !3;
    let uv_offset = row_stride.checked_mul(height)?;
    let uv_size = row_stride.checked_mul(height / 2)?;
    let size_bytes = uv_offset.checked_add(uv_size)?;
    Some((row_stride, uv_offset, size_bytes))
}

fn nv12_push_constants(width: u32, height: u32, row_stride: u32) -> PackedByteArray {
    let bytes = nv12_push_constant_bytes(width, height, row_stride);
    PackedByteArray::from(bytes.as_slice())
}

fn nv12_push_constant_bytes(width: u32, height: u32, row_stride: u32) -> [u8; 16] {
    let uv_offset = row_stride * height;
    let values = [width, height, row_stride / 4, uv_offset / 4];
    let mut bytes = [0_u8; 16];
    for (chunk, value) in bytes.chunks_exact_mut(4).zip(values) {
        chunk.copy_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn validate_readback_layout(
    format: PixelFormat,
    width: u32,
    height: u32,
    declared_stride: u32,
    len: usize,
) -> Option<u32> {
    match format {
        PixelFormat::Nv12 => {
            let (expected_stride, _, expected_len) = nv12_layout(width, height)?;
            (declared_stride == expected_stride && len == expected_len as usize)
                .then_some(expected_stride)
        }
        PixelFormat::Rgba8 | PixelFormat::Bgra8 => {
            let bytes_per_pixel = format.packed_bytes_per_pixel()? as u32;
            let minimum_stride = width.checked_mul(bytes_per_pixel)?;
            if height == 0 || !len.is_multiple_of(height as usize) {
                return None;
            }
            let actual_stride = u32::try_from(len / height as usize).ok()?;
            (actual_stride >= minimum_stride).then_some(actual_stride)
        }
    }
}

struct DownscaleOutput {
    readback: AsyncReadback,
    slot: usize,
    format: PixelFormat,
    row_stride: u32,
}

enum DownscaleDispatch {
    Ready(DownscaleOutput),
    Disabled,
    Busy,
}

fn dispatch_gpu_preprocess(
    rd: &mut Gd<RenderingDevice>,
    state: &Arc<SharedState>,
    source_texture: Rid,
    output_width: u32,
    output_height: u32,
) -> DownscaleDispatch {
    let mut downscale = state.downscale.lock().expect("downscale mutex");
    let dimensions_changed = matches!(
        &*downscale,
        DownscaleState::Ready(resources)
            if resources.output_width != output_width || resources.output_height != output_height
    );
    if dimensions_changed {
        let DownscaleState::Ready(resources) = &*downscale else {
            unreachable!();
        };
        if resources.busy.iter().any(|busy| *busy) {
            return DownscaleDispatch::Busy;
        }
        let old = std::mem::replace(&mut *downscale, DownscaleState::Uninitialized);
        free_downscale_state(rd, old);
    }
    if matches!(*downscale, DownscaleState::Uninitialized) {
        *downscale = match create_preprocess_resources(
            rd,
            source_texture,
            output_width,
            output_height,
        ) {
            Ok(resources) => {
                match resources.readback_kind {
                        GpuReadbackKind::Nv12Buffer {
                            row_stride,
                            size_bytes,
                        } => godot_print!(
                            "game_stream: GPU NV12 preprocess enabled ({}x{}, stride {}, {:.2} MiB/frame, {} readback buffers)",
                            output_width,
                            output_height,
                            row_stride,
                            size_bytes as f64 / (1024.0 * 1024.0),
                            DOWNSCALE_BUFFER_COUNT,
                        ),
                        GpuReadbackKind::RgbaTexture => godot_print!(
                            "game_stream: GPU RGBA pre-downscale fallback enabled ({}x{}, {} readback buffers)",
                            output_width,
                            output_height,
                            DOWNSCALE_BUFFER_COUNT,
                        ),
                    }
                DownscaleState::Ready(resources)
            }
            Err(error) => {
                godot_warn!(
                    "game_stream: GPU preprocess unavailable, using source RGBA readback: {error}"
                );
                DownscaleState::Disabled
            }
        };
    }

    let result = (|| -> Result<DownscaleOutput, String> {
        let DownscaleState::Ready(resources) = &mut *downscale else {
            return Err("disabled".into());
        };

        let Some(slot) = (0..DOWNSCALE_BUFFER_COUNT)
            .map(|offset| (resources.next_slot + offset) % DOWNSCALE_BUFFER_COUNT)
            .find(|slot| !resources.busy[*slot])
        else {
            return Err("busy".into());
        };

        if resources.source_textures[slot] != source_texture
            || !rd.uniform_set_is_valid(resources.uniform_sets[slot])
        {
            if resources.uniform_sets[slot].is_valid()
                && rd.uniform_set_is_valid(resources.uniform_sets[slot])
            {
                rd.free_rid(resources.uniform_sets[slot]);
            }
            resources.uniform_sets[slot] = create_preprocess_uniform_set(
                rd,
                resources.shader,
                resources.sampler,
                source_texture,
                resources.output_resources[slot],
                resources.readback_kind,
            )?;
            resources.source_textures[slot] = source_texture;
        }

        let output_resource = resources.output_resources[slot];
        let output_valid = match resources.readback_kind {
            GpuReadbackKind::Nv12Buffer { .. } => output_resource.is_valid(),
            GpuReadbackKind::RgbaTexture => rd.texture_is_valid(output_resource),
        };
        if !rd.compute_pipeline_is_valid(resources.pipeline)
            || !rd.uniform_set_is_valid(resources.uniform_sets[slot])
            || !output_valid
        {
            return Err("one or more RD resources became invalid".into());
        }
        let rgba_row_stride = output_width
            .checked_mul(4)
            .ok_or_else(|| "RGBA stride overflow".to_string())?;

        let compute_list = rd.compute_list_begin();
        if compute_list < 0 {
            return Err("compute_list_begin failed".into());
        }
        rd.compute_list_bind_compute_pipeline(compute_list, resources.pipeline);
        rd.compute_list_bind_uniform_set(compute_list, resources.uniform_sets[slot], 0);
        let output = match resources.readback_kind {
            GpuReadbackKind::Nv12Buffer {
                row_stride,
                size_bytes: _,
            } => {
                let push_constants = nv12_push_constants(output_width, output_height, row_stride);
                rd.compute_list_set_push_constant(
                    compute_list,
                    &push_constants,
                    push_constants.len() as u32,
                );
                rd.compute_list_dispatch(
                    compute_list,
                    output_width.div_ceil(DOWNSCALE_WORKGROUP_SIZE * NV12_PIXELS_PER_INVOCATION_X),
                    output_height.div_ceil(DOWNSCALE_WORKGROUP_SIZE * NV12_PIXELS_PER_INVOCATION_Y),
                    1,
                );
                DownscaleOutput {
                    readback: AsyncReadback::Buffer(output_resource),
                    slot,
                    format: PixelFormat::Nv12,
                    row_stride,
                }
            }
            GpuReadbackKind::RgbaTexture => {
                rd.compute_list_dispatch(
                    compute_list,
                    output_width.div_ceil(DOWNSCALE_WORKGROUP_SIZE),
                    output_height.div_ceil(DOWNSCALE_WORKGROUP_SIZE),
                    1,
                );
                DownscaleOutput {
                    readback: AsyncReadback::Texture(output_resource),
                    slot,
                    format: PixelFormat::Rgba8,
                    row_stride: rgba_row_stride,
                }
            }
        };
        rd.compute_list_end();
        resources.busy[slot] = true;
        resources.next_slot = (slot + 1) % DOWNSCALE_BUFFER_COUNT;
        Ok(output)
    })();

    match result {
        Ok(output) => {
            state.gpu_downscale_active.store(true, Ordering::Relaxed);
            state.gpu_downscale_frames.fetch_add(1, Ordering::Relaxed);
            if output.format == PixelFormat::Nv12 {
                state.gpu_nv12_frames.fetch_add(1, Ordering::Relaxed);
            }
            DownscaleDispatch::Ready(output)
        }
        Err(error) if error == "disabled" => DownscaleDispatch::Disabled,
        Err(error) if error == "busy" => DownscaleDispatch::Busy,
        Err(error) => {
            godot_warn!("game_stream: disabling GPU pre-downscale: {error}");
            let old = std::mem::replace(&mut *downscale, DownscaleState::Disabled);
            free_downscale_state(rd, old);
            state.gpu_downscale_active.store(false, Ordering::Relaxed);
            DownscaleDispatch::Disabled
        }
    }
}

fn create_preprocess_resources(
    rd: &mut Gd<RenderingDevice>,
    source_texture: Rid,
    output_width: u32,
    output_height: u32,
) -> Result<DownscaleResources, String> {
    match create_nv12_preprocess_resources(rd, source_texture, output_width, output_height) {
        Ok(resources) => Ok(resources),
        Err(nv12_error) => {
            create_rgba_preprocess_resources(rd, source_texture, output_width, output_height)
                .map_err(|rgba_error| {
                    format!("NV12 path failed ({nv12_error}); RGBA fallback failed ({rgba_error})")
                })
        }
    }
}

fn create_nv12_preprocess_resources(
    rd: &mut Gd<RenderingDevice>,
    source_texture: Rid,
    output_width: u32,
    output_height: u32,
) -> Result<DownscaleResources, String> {
    let Some((row_stride, _, size_bytes)) = nv12_layout(output_width, output_height) else {
        return Err("invalid NV12 output layout".into());
    };
    create_preprocess_resources_for_kind(
        rd,
        source_texture,
        output_width,
        output_height,
        GpuReadbackKind::Nv12Buffer {
            row_stride,
            size_bytes,
        },
        NV12_PREPROCESS_SHADER,
        "game_stream_nv12_preprocess",
    )
}

fn create_rgba_preprocess_resources(
    rd: &mut Gd<RenderingDevice>,
    source_texture: Rid,
    output_width: u32,
    output_height: u32,
) -> Result<DownscaleResources, String> {
    let usage = TextureUsageBits::STORAGE_BIT | TextureUsageBits::CAN_COPY_FROM_BIT;
    if !rd.texture_is_format_supported_for_usage(DataFormat::R8G8B8A8_UNORM, usage) {
        return Err("RGBA8 storage/readback texture is unsupported".into());
    }
    create_preprocess_resources_for_kind(
        rd,
        source_texture,
        output_width,
        output_height,
        GpuReadbackKind::RgbaTexture,
        RGBA_DOWNSCALE_SHADER,
        "game_stream_rgba_preprocess",
    )
}

#[allow(clippy::too_many_arguments)]
fn create_preprocess_resources_for_kind(
    rd: &mut Gd<RenderingDevice>,
    source_texture: Rid,
    output_width: u32,
    output_height: u32,
    readback_kind: GpuReadbackKind,
    shader_code: &str,
    shader_name: &str,
) -> Result<DownscaleResources, String> {
    let output_resource_0 =
        create_preprocess_output(rd, 0, readback_kind, output_width, output_height)?;
    let output_resource_1 =
        match create_preprocess_output(rd, 1, readback_kind, output_width, output_height) {
            Ok(resource) => resource,
            Err(error) => {
                free_preprocess_build(
                    rd,
                    &[output_resource_0],
                    readback_kind,
                    &[],
                    None,
                    None,
                    None,
                );
                return Err(error);
            }
        };
    let output_resources = [output_resource_0, output_resource_1];

    let mut shader_source = RdShaderSource::new_gd();
    shader_source.set_stage_source(ShaderStage::COMPUTE, shader_code);
    let Some(spirv) = rd.shader_compile_spirv_from_source(&shader_source) else {
        free_preprocess_build(rd, &output_resources, readback_kind, &[], None, None, None);
        return Err("shader_compile_spirv_from_source returned null".into());
    };
    let compile_error = spirv.get_stage_compile_error(ShaderStage::COMPUTE);
    if !compile_error.is_empty() {
        free_preprocess_build(rd, &output_resources, readback_kind, &[], None, None, None);
        return Err(format!("compute shader compile failed: {compile_error}"));
    }
    let shader = rd
        .shader_create_from_spirv_ex(&spirv)
        .name(shader_name)
        .done();
    if !shader.is_valid() {
        free_preprocess_build(rd, &output_resources, readback_kind, &[], None, None, None);
        return Err("shader_create_from_spirv failed".into());
    }
    let pipeline = rd.compute_pipeline_create(shader);
    if !pipeline.is_valid() || !rd.compute_pipeline_is_valid(pipeline) {
        free_preprocess_build(
            rd,
            &output_resources,
            readback_kind,
            &[],
            Some(shader),
            None,
            None,
        );
        return Err("compute_pipeline_create failed".into());
    }

    let mut sampler_state = RdSamplerState::new_gd();
    sampler_state.set_mag_filter(SamplerFilter::LINEAR);
    sampler_state.set_min_filter(SamplerFilter::LINEAR);
    sampler_state.set_mip_filter(SamplerFilter::LINEAR);
    sampler_state.set_repeat_u(SamplerRepeatMode::CLAMP_TO_EDGE);
    sampler_state.set_repeat_v(SamplerRepeatMode::CLAMP_TO_EDGE);
    sampler_state.set_repeat_w(SamplerRepeatMode::CLAMP_TO_EDGE);
    let sampler = rd.sampler_create(&sampler_state);
    if !sampler.is_valid() {
        free_preprocess_build(
            rd,
            &output_resources,
            readback_kind,
            &[],
            Some(shader),
            Some(pipeline),
            None,
        );
        return Err("sampler_create failed".into());
    }

    let uniform_set_0 = match create_preprocess_uniform_set(
        rd,
        shader,
        sampler,
        source_texture,
        output_resource_0,
        readback_kind,
    ) {
        Ok(uniform_set) => uniform_set,
        Err(error) => {
            free_preprocess_build(
                rd,
                &output_resources,
                readback_kind,
                &[],
                Some(shader),
                Some(pipeline),
                Some(sampler),
            );
            return Err(error);
        }
    };
    let uniform_set_1 = match create_preprocess_uniform_set(
        rd,
        shader,
        sampler,
        source_texture,
        output_resource_1,
        readback_kind,
    ) {
        Ok(uniform_set) => uniform_set,
        Err(error) => {
            free_preprocess_build(
                rd,
                &output_resources,
                readback_kind,
                &[uniform_set_0],
                Some(shader),
                Some(pipeline),
                Some(sampler),
            );
            return Err(error);
        }
    };

    Ok(DownscaleResources {
        output_width,
        output_height,
        readback_kind,
        shader,
        pipeline,
        sampler,
        output_resources,
        uniform_sets: [uniform_set_0, uniform_set_1],
        source_textures: [source_texture; DOWNSCALE_BUFFER_COUNT],
        busy: [false; DOWNSCALE_BUFFER_COUNT],
        next_slot: 0,
    })
}

fn create_preprocess_output(
    rd: &mut Gd<RenderingDevice>,
    slot: usize,
    readback_kind: GpuReadbackKind,
    output_width: u32,
    output_height: u32,
) -> Result<Rid, String> {
    match readback_kind {
        GpuReadbackKind::Nv12Buffer { size_bytes, .. } => {
            let output_buffer = rd.storage_buffer_create(size_bytes);
            if !output_buffer.is_valid() {
                return Err("storage_buffer_create failed".into());
            }
            let name = format!("game_stream_{output_width}x{output_height}_nv12_{slot}");
            rd.set_resource_name(output_buffer, &name);
            Ok(output_buffer)
        }
        GpuReadbackKind::RgbaTexture => {
            let usage = TextureUsageBits::STORAGE_BIT | TextureUsageBits::CAN_COPY_FROM_BIT;
            create_downscale_texture(rd, slot, usage, output_width, output_height)
        }
    }
}

fn create_downscale_texture(
    rd: &mut Gd<RenderingDevice>,
    slot: usize,
    usage: TextureUsageBits,
    output_width: u32,
    output_height: u32,
) -> Result<Rid, String> {
    let mut texture_format = RdTextureFormat::new_gd();
    texture_format.set_format(DataFormat::R8G8B8A8_UNORM);
    texture_format.set_width(output_width);
    texture_format.set_height(output_height);
    texture_format.set_depth(1);
    texture_format.set_array_layers(1);
    texture_format.set_mipmaps(1);
    texture_format.set_texture_type(TextureType::TYPE_2D);
    texture_format.set_samples(TextureSamples::SAMPLES_1);
    texture_format.set_usage_bits(usage);
    let texture_view = RdTextureView::new_gd();
    let output_texture = rd.texture_create(&texture_format, &texture_view);
    if !output_texture.is_valid() {
        return Err("texture_create failed".into());
    }
    let name = format!("game_stream_{output_width}x{output_height}_readback_{slot}");
    rd.set_resource_name(output_texture, &name);
    Ok(output_texture)
}

fn free_preprocess_build(
    rd: &mut Gd<RenderingDevice>,
    output_resources: &[Rid],
    readback_kind: GpuReadbackKind,
    uniform_sets: &[Rid],
    shader: Option<Rid>,
    pipeline: Option<Rid>,
    sampler: Option<Rid>,
) {
    for uniform_set in uniform_sets {
        if uniform_set.is_valid() && rd.uniform_set_is_valid(*uniform_set) {
            rd.free_rid(*uniform_set);
        }
    }
    if let Some(pipeline) = pipeline {
        if pipeline.is_valid() && rd.compute_pipeline_is_valid(pipeline) {
            rd.free_rid(pipeline);
        }
    }
    if let Some(sampler) = sampler {
        if sampler.is_valid() {
            rd.free_rid(sampler);
        }
    }
    if let Some(shader) = shader {
        if shader.is_valid() {
            rd.free_rid(shader);
        }
    }
    for output_resource in output_resources {
        let is_valid = match readback_kind {
            GpuReadbackKind::Nv12Buffer { .. } => output_resource.is_valid(),
            GpuReadbackKind::RgbaTexture => {
                output_resource.is_valid() && rd.texture_is_valid(*output_resource)
            }
        };
        if is_valid {
            rd.free_rid(*output_resource);
        }
    }
}

fn create_preprocess_uniform_set(
    rd: &mut Gd<RenderingDevice>,
    shader: Rid,
    sampler: Rid,
    source_texture: Rid,
    output_resource: Rid,
    readback_kind: GpuReadbackKind,
) -> Result<Rid, String> {
    let mut source_uniform = RdUniform::new_gd();
    source_uniform.set_uniform_type(UniformType::SAMPLER_WITH_TEXTURE);
    source_uniform.set_binding(0);
    source_uniform.add_id(sampler);
    source_uniform.add_id(source_texture);

    let mut output_uniform = RdUniform::new_gd();
    output_uniform.set_uniform_type(match readback_kind {
        GpuReadbackKind::Nv12Buffer { .. } => UniformType::STORAGE_BUFFER,
        GpuReadbackKind::RgbaTexture => UniformType::IMAGE,
    });
    output_uniform.set_binding(1);
    output_uniform.add_id(output_resource);

    let mut uniforms = Array::new();
    uniforms.push(&source_uniform);
    uniforms.push(&output_uniform);
    let uniform_set = rd.uniform_set_create(&uniforms, shader, 0);
    if !uniform_set.is_valid() || !rd.uniform_set_is_valid(uniform_set) {
        return Err("uniform_set_create failed".into());
    }
    Ok(uniform_set)
}

fn release_downscale_slot(state: &Arc<SharedState>, slot: Option<usize>) {
    let Some(slot) = slot else {
        return;
    };
    let mut downscale = state.downscale.lock().expect("downscale mutex");
    if let DownscaleState::Ready(resources) = &mut *downscale {
        if slot < DOWNSCALE_BUFFER_COUNT {
            resources.busy[slot] = false;
        }
    }
}

fn finish_in_flight(state: &Arc<SharedState>) {
    // Saturating decrement: a self-heal reset may have already zeroed the
    // counter while this completion was still in flight.
    loop {
        let current = state.in_flight.load(Ordering::SeqCst);
        if current == 0 {
            break;
        }
        if state
            .in_flight
            .compare_exchange(current, current - 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            *state
                .in_flight_stuck_since
                .lock()
                .expect("in_flight watch mutex") = None;
            if current == 1 {
                cleanup_if_idle(state);
            }
            break;
        }
    }
}

/// Recovers `in_flight` slots whose readback completion never arrived (e.g. a
/// Godot-side callback dropped during viewport churn). Without this, two lost
/// callbacks would leave the capture gate permanently closed: the game keeps
/// rendering while clients see a frozen stream.
const IN_FLIGHT_STUCK_TIMEOUT: Duration = Duration::from_millis(500);

fn recover_stuck_in_flight(state: &Arc<SharedState>) {
    let mut stuck = state
        .in_flight_stuck_since
        .lock()
        .expect("in_flight watch mutex");
    let Some(stuck_since) = *stuck else {
        *stuck = Some(Instant::now());
        return;
    };
    if stuck_since.elapsed() < IN_FLIGHT_STUCK_TIMEOUT {
        return;
    }
    let leaked = state.in_flight.swap(0, Ordering::SeqCst);
    *stuck = None;
    drop(stuck);
    godot_warn!(
        "game_stream: recovered {leaked} stuck readback slot(s) after {:.0}ms without completions",
        stuck_since.elapsed().as_millis()
    );
    state.force_keyframe.store(true, Ordering::SeqCst);
}

fn cleanup_if_idle(state: &Arc<SharedState>) {
    if !state.cleanup_requested.load(Ordering::SeqCst)
        || state.in_flight.load(Ordering::SeqCst) != 0
    {
        return;
    }
    let Some(mut rd) = RenderingServer::singleton().get_rendering_device() else {
        return;
    };
    let old = {
        let mut downscale = state.downscale.lock().expect("downscale mutex");
        std::mem::replace(&mut *downscale, DownscaleState::Disabled)
    };
    free_downscale_state(&mut rd, old);
    state.gpu_downscale_active.store(false, Ordering::Relaxed);
}

fn free_downscale_state(rd: &mut Gd<RenderingDevice>, state: DownscaleState) {
    let DownscaleState::Ready(resources) = state else {
        return;
    };
    free_preprocess_build(
        rd,
        &resources.output_resources,
        resources.readback_kind,
        &resources.uniform_sets,
        Some(resources.shader),
        Some(resources.pipeline),
        Some(resources.sampler),
    );
}

fn duration_to_pts(duration: Duration) -> i64 {
    let ticks = duration.as_nanos().saturating_mul(VIDEO_TIME_BASE as u128) / 1_000_000_000;
    ticks.min(i64::MAX as u128) as i64
}

fn map_rd_format(format: DataFormat) -> Option<PixelFormat> {
    match format {
        DataFormat::R8G8B8A8_UNORM | DataFormat::R8G8B8A8_SRGB => Some(PixelFormat::Rgba8),
        DataFormat::B8G8R8A8_UNORM | DataFormat::B8G8R8A8_SRGB => Some(PixelFormat::Bgra8),
        _ => None,
    }
}

fn log_unsupported_format(format: DataFormat) {
    use std::sync::OnceLock;
    static LAST: OnceLock<std::sync::Mutex<Option<i64>>> = OnceLock::new();
    let ord = i64::from(format.ord());
    let cache = LAST.get_or_init(|| std::sync::Mutex::new(None));
    let mut last = cache.lock().expect("format log mutex");
    if *last == Some(ord) {
        return;
    }
    *last = Some(ord);
    godot_warn!("game_stream: unsupported RD format {format:?}");
}

pub fn rendering_device_available() -> bool {
    RenderingServer::singleton()
        .get_rendering_device()
        .is_some()
}

pub fn root_viewport_rid() -> Option<Rid> {
    let tree = current_scene_tree()?;
    let root = tree.get_root()?;
    let rid = root.get_viewport_rid();
    rid.is_valid().then_some(rid)
}

#[allow(dead_code)]
fn _rd_ty(_: Gd<RenderingDevice>) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nv12_layout_is_even_and_four_byte_aligned() {
        assert_eq!(nv12_layout(1920, 1080), Some((1920, 2_073_600, 3_110_400)));
        assert_eq!(nv12_layout(1918, 1080), Some((1920, 2_073_600, 3_110_400)));
        assert_eq!(nv12_layout(1919, 1080), None);
        assert_eq!(nv12_layout(1920, 1079), None);
    }

    #[test]
    fn nv12_push_constants_match_shader_layout() {
        let bytes = nv12_push_constant_bytes(1918, 1080, 1920);
        let values: Vec<u32> = bytes
            .chunks_exact(4)
            .map(|chunk| u32::from_le_bytes(chunk.try_into().expect("four bytes")))
            .collect();
        assert_eq!(values, [1918, 1080, 480, 518_400]);
    }

    #[test]
    fn readback_layout_rejects_truncated_or_misdeclared_planes() {
        assert_eq!(
            validate_readback_layout(PixelFormat::Nv12, 1918, 1080, 1920, 3_110_400),
            Some(1920)
        );
        assert_eq!(
            validate_readback_layout(PixelFormat::Nv12, 1918, 1080, 1918, 3_110_400),
            None
        );
        assert_eq!(
            validate_readback_layout(PixelFormat::Nv12, 1918, 1080, 1920, 3_110_399),
            None
        );
        assert_eq!(
            validate_readback_layout(PixelFormat::Rgba8, 4, 2, 16, 32),
            Some(16)
        );
    }

    #[test]
    fn forced_redraw_waits_for_a_pending_keyframe_and_a_quiet_game() {
        let now = 10_000_000_000;
        // No pending keyframe: never force, even when the game is idle.
        assert!(!should_force_draw(false, 0, 0, now));
        // Pending keyframe but the game still draws every frame: not needed.
        assert!(!should_force_draw(true, now - 16_000, 0, now));
        // Pending keyframe and the game went quiet: force one redraw.
        assert!(should_force_draw(
            true,
            now - FORCED_DRAW_MIN_INTERVAL_US,
            0,
            now
        ));
        // Never drew since streaming started: the startup bootstrap forces.
        assert!(should_force_draw(true, 0, 0, now));
        // A recent forced redraw is rate-limited until the interval elapses.
        assert!(!should_force_draw(
            true,
            now - FORCED_DRAW_MIN_INTERVAL_US * 5,
            now - 16_000,
            now
        ));
        assert!(should_force_draw(
            true,
            now - FORCED_DRAW_MIN_INTERVAL_US * 5,
            now - FORCED_DRAW_MIN_INTERVAL_US,
            now
        ));
    }

    #[test]
    fn shader_uses_bt709_studio_range_reference_values() {
        fn convert(rgb: [f32; 3]) -> [u8; 3] {
            let byte = |value: f32| (value * 255.0).round().clamp(0.0, 255.0) as u8;
            [
                byte(16.0 / 255.0 + rgb[0] * 0.182_586 + rgb[1] * 0.614_231 + rgb[2] * 0.062_007),
                byte(128.0 / 255.0 - rgb[0] * 0.100_644 - rgb[1] * 0.338_572 + rgb[2] * 0.439_216),
                byte(128.0 / 255.0 + rgb[0] * 0.439_216 - rgb[1] * 0.398_942 - rgb[2] * 0.040_274),
            ]
        }

        assert_eq!(convert([0.0, 0.0, 0.0]), [16, 128, 128]);
        assert_eq!(convert([1.0, 1.0, 1.0]), [235, 128, 128]);
        assert_eq!(convert([1.0, 0.0, 0.0]), [63, 102, 240]);
    }
}
