// SPDX-License-Identifier: LGPL-2.1-or-later
// Copyright (C) 2026 yearsyan and contributors

#include "decoder.h"

#include <libavcodec/packet.h>
#include <libavutil/error.h>
#include <libavutil/hwcontext.h>
#include <libavutil/mem.h>
#include <libavutil/pixdesc.h>
#include <limits.h>
#include <stdio.h>
#include <string.h>

static bool mc_decoder_fail(struct mc_decoder* decoder, const char* operation, int error) {
    char detail[AV_ERROR_MAX_STRING_SIZE];
    av_strerror(error, detail, sizeof(detail));
    snprintf(decoder->error, sizeof(decoder->error), "%.20s failed: %.32s", operation, detail);
    return false;
}

static void mc_decoder_clear_replay_packets(struct mc_decoder* decoder) {
    for (size_t index = 0; index < decoder->replay_count; ++index) {
        av_packet_unref(&decoder->replay_packets[index]);
    }
    av_freep(&decoder->replay_packets);
    decoder->replay_count = 0;
    decoder->replay_capacity = 0;
}

static bool
mc_decoder_queue_replay_packet(struct mc_decoder* decoder, const uint8_t* data, int size) {
    if (decoder->replay_count == decoder->replay_capacity) {
        if (decoder->replay_capacity > SIZE_MAX / 2) {
            snprintf(
                decoder->error, sizeof(decoder->error), "decoder fallback packet buffer overflow");
            return false;
        }
        size_t capacity = decoder->replay_capacity ? decoder->replay_capacity * 2 : 8;
        if (capacity > SIZE_MAX / sizeof(AVPacket)) {
            snprintf(
                decoder->error, sizeof(decoder->error), "decoder fallback packet buffer overflow");
            return false;
        }
        AVPacket* packets = av_realloc_array(decoder->replay_packets, capacity, sizeof(*packets));
        if (!packets) {
            snprintf(decoder->error,
                     sizeof(decoder->error),
                     "could not buffer packets for decoder fallback");
            return false;
        }
        memset(packets + decoder->replay_capacity,
               0,
               (capacity - decoder->replay_capacity) * sizeof(*packets));
        decoder->replay_packets = packets;
        decoder->replay_capacity = capacity;
    }

    AVPacket* packet = &decoder->replay_packets[decoder->replay_count];
    int result = av_new_packet(packet, size);
    if (result < 0) {
        return mc_decoder_fail(decoder, "av_new_packet", result);
    }
    memcpy(packet->data, data, (size_t)size);
    ++decoder->replay_count;
    return true;
}

static enum AVPixelFormat mc_decoder_get_hw_format(AVCodecContext* context,
                                                   const enum AVPixelFormat* pixel_formats) {
    const struct mc_decoder* decoder = context->opaque;
    if (!decoder) {
        return AV_PIX_FMT_NONE;
    }
    for (const enum AVPixelFormat* pixel = pixel_formats; *pixel != AV_PIX_FMT_NONE; ++pixel) {
        if (*pixel == decoder->hw_pix_fmt) {
            return *pixel;
        }
    }
    return AV_PIX_FMT_NONE;
}

static void mc_decoder_copy_color(AVFrame* destination, const AVFrame* source) {
    destination->color_range = source->color_range;
    destination->colorspace = source->colorspace;
    destination->color_primaries = source->color_primaries;
    destination->color_trc = source->color_trc;
}

static int mc_hw_device_priority(enum AVHWDeviceType type) {
    switch (type) {
    case AV_HWDEVICE_TYPE_D3D11VA:
        return 100;
    case AV_HWDEVICE_TYPE_D3D12VA:
        return 90;
    case AV_HWDEVICE_TYPE_VIDEOTOOLBOX:
        return 80;
    case AV_HWDEVICE_TYPE_CUDA:
        return 70;
    case AV_HWDEVICE_TYPE_VAAPI:
        return 60;
    case AV_HWDEVICE_TYPE_DXVA2:
        return 50;
    default:
        return 10;
    }
}

static bool mc_decoder_try_hw_device(struct mc_decoder* decoder, const AVCodec* codec) {
    int best_priority = 0;
    enum AVHWDeviceType best_type = AV_HWDEVICE_TYPE_NONE;
    enum AVPixelFormat best_pix_fmt = AV_PIX_FMT_NONE;
    AVBufferRef* best_device = NULL;

    for (int index = 0;; ++index) {
        const AVCodecHWConfig* config = avcodec_get_hw_config(codec, index);
        if (!config) {
            break;
        }
        if (!(config->methods & AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX) ||
            config->device_type == AV_HWDEVICE_TYPE_NONE) {
            continue;
        }

        int priority = mc_hw_device_priority(config->device_type);
        if (priority <= best_priority) {
            continue;
        }

        AVBufferRef* device = NULL;
        if (av_hwdevice_ctx_create(&device, config->device_type, NULL, NULL, 0) < 0) {
            continue;
        }
        av_buffer_unref(&best_device);
        best_device = device;
        best_priority = priority;
        best_type = config->device_type;
        best_pix_fmt = config->pix_fmt;
    }

    if (!best_device) {
        return false;
    }

    decoder->hw_device = best_device;
    decoder->hw_pix_fmt = best_pix_fmt;
    fprintf(stderr,
            "decoder: trying %s hwaccel %s (%s)\n",
            codec->name,
            av_hwdevice_get_type_name(best_type),
            av_get_pix_fmt_name(best_pix_fmt));
    return true;
}

static void mc_decoder_close_codec(struct mc_decoder* decoder) {
    av_frame_free(&decoder->frame);
    av_frame_free(&decoder->sw_frame);
    avcodec_free_context(&decoder->codec);
    av_buffer_unref(&decoder->hw_device);
    decoder->hw_pix_fmt = AV_PIX_FMT_NONE;
    decoder->hardware_active = false;
    decoder->hardware_confirmed = false;
}

static bool mc_decoder_open_codec(struct mc_decoder* decoder, const AVCodec* codec, bool hardware) {
    decoder->codec = avcodec_alloc_context3(codec);
    decoder->frame = av_frame_alloc();
    if (hardware) {
        decoder->sw_frame = av_frame_alloc();
    }
    if (!decoder->codec || !decoder->frame || (hardware && !decoder->sw_frame)) {
        snprintf(decoder->error, sizeof(decoder->error), "could not allocate FFmpeg decoder state");
        mc_decoder_close_codec(decoder);
        return false;
    }

    decoder->codec->opaque = decoder;
    decoder->codec->flags |= AV_CODEC_FLAG_LOW_DELAY;
    decoder->codec->flags2 |= AV_CODEC_FLAG2_FAST;
    if (hardware) {
        decoder->codec->get_format = mc_decoder_get_hw_format;
        decoder->codec->hw_device_ctx = av_buffer_ref(decoder->hw_device);
        if (!decoder->codec->hw_device_ctx) {
            snprintf(decoder->error, sizeof(decoder->error), "could not reference HW device");
            mc_decoder_close_codec(decoder);
            return false;
        }
        decoder->codec->extra_hw_frames = 8;
        decoder->codec->thread_count = 1;
    } else if (codec->id == AV_CODEC_ID_AV1) {
        // libdav1d auto-threads; LOW_DELAY pins max_frame_delay to 1.
        decoder->codec->thread_count = 0;
    } else {
        decoder->codec->thread_count = 1;
        decoder->codec->thread_type = FF_THREAD_SLICE;
    }

    int result = avcodec_open2(decoder->codec, codec, NULL);
    if (result < 0) {
        mc_decoder_fail(decoder, "avcodec_open2", result);
        mc_decoder_close_codec(decoder);
        return false;
    }
    decoder->hardware_active = hardware;
    decoder->hardware_confirmed = false;
    decoder->error[0] = '\0';
    return true;
}

static bool mc_decoder_drain(struct mc_decoder* decoder) {
    for (;;) {
        int result = avcodec_receive_frame(decoder->codec, decoder->frame);
        if (result == AVERROR(EAGAIN) || result == AVERROR_EOF) {
            return true;
        }
        if (result < 0) {
            return mc_decoder_fail(decoder, "avcodec_receive_frame", result);
        }

        const AVFrame* output = decoder->frame;
        bool hardware_frame = decoder->hw_device && decoder->frame->format == decoder->hw_pix_fmt;
        if (hardware_frame) {
            av_frame_unref(decoder->sw_frame);
            result = av_hwframe_transfer_data(decoder->sw_frame, decoder->frame, 0);
            if (result < 0) {
                av_frame_unref(decoder->frame);
                return mc_decoder_fail(decoder, "av_hwframe_transfer_data", result);
            }
            mc_decoder_copy_color(decoder->sw_frame, decoder->frame);
            output = decoder->sw_frame;
        }

        if (decoder->hardware_active && !decoder->hardware_confirmed) {
            decoder->hardware_confirmed = true;
            mc_decoder_clear_replay_packets(decoder);
            fprintf(stderr,
                    "decoder: %s %s output confirmed (%s)\n",
                    decoder->codec->codec->name,
                    hardware_frame ? "hardware" : "software",
                    av_get_pix_fmt_name((enum AVPixelFormat)output->format));
        }

        bool accepted = decoder->on_frame(decoder->callback_userdata, output);
        av_frame_unref(decoder->frame);
        if (decoder->sw_frame) {
            av_frame_unref(decoder->sw_frame);
        }
        if (!accepted) {
            snprintf(decoder->error, sizeof(decoder->error), "video frame sink stopped");
            return false;
        }
    }
}

static bool mc_decoder_send_packet_once(struct mc_decoder* decoder, const uint8_t* data, int size) {
    AVPacket packet;
    memset(&packet, 0, sizeof(packet));
    packet.data = (uint8_t*)data;
    packet.size = size;

    int result = avcodec_send_packet(decoder->codec, &packet);
    if (result == AVERROR(EAGAIN)) {
        if (!mc_decoder_drain(decoder)) {
            return false;
        }
        result = avcodec_send_packet(decoder->codec, &packet);
    }
    if (result < 0) {
        return mc_decoder_fail(decoder, "avcodec_send_packet", result);
    }
    return mc_decoder_drain(decoder);
}

static bool mc_decoder_fallback_to_software(struct mc_decoder* decoder) {
    if (!decoder->software_codec) {
        return false;
    }

    char hardware_error[AV_ERROR_MAX_STRING_SIZE];
    snprintf(hardware_error,
             sizeof(hardware_error),
             "%s",
             decoder->error[0] ? decoder->error : "unknown hardware decoder error");
    fprintf(stderr,
            "decoder: %s hwaccel failed before first frame (%s); falling back to %s\n",
            avcodec_get_name(decoder->software_codec->id),
            hardware_error,
            decoder->software_codec->name);

    mc_decoder_close_codec(decoder);
    if (!mc_decoder_open_codec(decoder, decoder->software_codec, false)) {
        mc_decoder_clear_replay_packets(decoder);
        return false;
    }

    bool ok = true;
    for (size_t index = 0; index < decoder->replay_count; ++index) {
        const AVPacket* packet = &decoder->replay_packets[index];
        if (!mc_decoder_send_packet_once(decoder, packet->data, packet->size)) {
            ok = false;
            break;
        }
    }
    mc_decoder_clear_replay_packets(decoder);
    if (!ok) {
        return false;
    }

    decoder->error[0] = '\0';
    fprintf(stderr,
            "decoder: %s software fallback active (%s)\n",
            avcodec_get_name(decoder->software_codec->id),
            decoder->software_codec->name);
    return true;
}

static bool mc_decoder_send_packet(struct mc_decoder* decoder, const uint8_t* data, int size) {
    if (decoder->hardware_active && !decoder->hardware_confirmed &&
        !mc_decoder_queue_replay_packet(decoder, data, size)) {
        return false;
    }
    if (mc_decoder_send_packet_once(decoder, data, size)) {
        return true;
    }
    if (decoder->hardware_active && !decoder->hardware_confirmed) {
        return mc_decoder_fallback_to_software(decoder);
    }
    return false;
}

// Cuts a low-overhead AV1 OBU stream into temporal units. A TD OBU starts a
// new temporal unit, so seeing one closes and emits the previous unit. Bytes
// before the first TD are dropped, which resyncs a mid-stream join.
static bool mc_decoder_push_av1(struct mc_decoder* decoder, const uint8_t* data, size_t size) {
    if (size > SIZE_MAX - decoder->av1_size) {
        snprintf(decoder->error, sizeof(decoder->error), "AV1 reassembly buffer overflow");
        return false;
    }
    size_t needed = decoder->av1_size + size;
    if (needed > decoder->av1_capacity) {
        size_t capacity = decoder->av1_capacity ? decoder->av1_capacity : 256 * 1024;
        while (capacity < needed) {
            capacity *= 2;
        }
        uint8_t* buffer = av_realloc(decoder->av1_buffer, capacity);
        if (!buffer) {
            snprintf(
                decoder->error, sizeof(decoder->error), "could not grow AV1 reassembly buffer");
            return false;
        }
        decoder->av1_buffer = buffer;
        decoder->av1_capacity = capacity;
    }
    memcpy(decoder->av1_buffer + decoder->av1_size, data, size);
    decoder->av1_size += size;

    const uint8_t* buffer = decoder->av1_buffer;
    while (decoder->av1_scan < decoder->av1_size) {
        size_t obu = decoder->av1_scan;
        uint8_t header = buffer[obu];
        if (header & 0x80) {
            snprintf(decoder->error, sizeof(decoder->error), "AV1 OBU forbidden bit set");
            return false;
        }
        unsigned type = (header >> 3) & 0x0f;
        size_t header_size = (header & 0x04) ? 2 : 1;
        if (!(header & 0x02)) {
            snprintf(decoder->error, sizeof(decoder->error), "AV1 OBU without size field");
            return false;
        }
        size_t cursor = obu + header_size;
        uint64_t payload_size = 0;
        unsigned shift = 0;
        bool complete = false;
        for (unsigned index = 0; index < 8; ++index) {
            if (cursor + index >= decoder->av1_size) {
                break;
            }
            uint8_t byte = buffer[cursor + index];
            payload_size |= (uint64_t)(byte & 0x7f) << shift;
            if (!(byte & 0x80)) {
                cursor += index + 1;
                complete = true;
                break;
            }
            shift += 7;
        }
        if (!complete) {
            break;
        }
        if (payload_size > decoder->av1_size - cursor) {
            break;
        }

        if (type == 2 && decoder->av1_tu_start != SIZE_MAX && decoder->av1_tu_start + 2 < obu) {
            size_t tu_size = obu - decoder->av1_tu_start;
            if (tu_size > INT_MAX ||
                !mc_decoder_send_packet(decoder, buffer + decoder->av1_tu_start, (int)tu_size)) {
                return false;
            }
        }
        if (type == 2) {
            decoder->av1_tu_start = obu;
        }
        decoder->av1_scan = cursor + (size_t)payload_size;
    }

    // Compact: emitted bytes precede the pending temporal unit.
    size_t keep = decoder->av1_tu_start != SIZE_MAX ? decoder->av1_tu_start : decoder->av1_scan;
    if (keep > 0) {
        memmove(decoder->av1_buffer, decoder->av1_buffer + keep, decoder->av1_size - keep);
        decoder->av1_size -= keep;
        decoder->av1_scan -= keep;
        decoder->av1_tu_start = decoder->av1_tu_start != SIZE_MAX ? 0 : SIZE_MAX;
    }
    return true;
}

bool mc_decoder_init(struct mc_decoder* decoder,
                     enum AVCodecID codec_id,
                     mc_decoder_frame_callback on_frame,
                     void* userdata) {
    memset(decoder, 0, sizeof(*decoder));
    decoder->on_frame = on_frame;
    decoder->callback_userdata = userdata;
    decoder->av1_tu_start = SIZE_MAX;
    decoder->hw_pix_fmt = AV_PIX_FMT_NONE;

    const AVCodec* native = avcodec_find_decoder(codec_id);
    if (codec_id == AV_CODEC_ID_AV1) {
        // Prefer the native av1 decoder for D3D11VA/VideoToolbox. libdav1d
        // shares AV_CODEC_ID_AV1 but has no hwaccel and would hide NVDEC.
        const AVCodec* av1 = avcodec_find_decoder_by_name("av1");
        if (av1) {
            native = av1;
        }
    }
    const AVCodec* software = native;
    if (codec_id == AV_CODEC_ID_AV1) {
        // FFmpeg's native av1 decoder refuses software decoding without a
        // working hwaccel (get_pixel_format returns ENOSYS), so software AV1
        // goes through libdav1d.
        software = avcodec_find_decoder_by_name("libdav1d");
    }
    decoder->software_codec = software;
    if (!native && !software) {
        snprintf(decoder->error,
                 sizeof(decoder->error),
                 "FFmpeg %s decoder is unavailable",
                 avcodec_get_name(codec_id));
        return false;
    }

    // The H.264 parser splits Annex-B into access units; the AV1 parser is
    // pass-through, so AV1 temporal units are framed manually in this file.
    if (codec_id != AV_CODEC_ID_AV1) {
        decoder->parser = av_parser_init(codec_id);
        if (!decoder->parser) {
            snprintf(
                decoder->error, sizeof(decoder->error), "could not allocate FFmpeg decoder state");
            return false;
        }
    }

    if (native && mc_decoder_try_hw_device(decoder, native) &&
        mc_decoder_open_codec(decoder, native, true)) {
        return true;
    }
    mc_decoder_close_codec(decoder);

    if (!software) {
        snprintf(decoder->error,
                 sizeof(decoder->error),
                 "FFmpeg %s decoder is unavailable",
                 avcodec_get_name(codec_id));
        mc_decoder_destroy(decoder);
        return false;
    }

    if (!mc_decoder_open_codec(decoder, software, false)) {
        mc_decoder_destroy(decoder);
        return false;
    }
    fprintf(stderr, "decoder: %s software (%s)\n", avcodec_get_name(codec_id), software->name);
    return true;
}

bool mc_decoder_push(struct mc_decoder* decoder, const uint8_t* data, size_t size) {
    if (!decoder->parser) {
        return mc_decoder_push_av1(decoder, data, size);
    }
    unsigned no_progress_count = 0;
    while (size > 0) {
        int chunk_size = size > INT_MAX ? INT_MAX : (int)size;
        uint8_t* packet_data = NULL;
        int packet_size = 0;
        int consumed = av_parser_parse2(decoder->parser,
                                        decoder->codec,
                                        &packet_data,
                                        &packet_size,
                                        data,
                                        chunk_size,
                                        AV_NOPTS_VALUE,
                                        AV_NOPTS_VALUE,
                                        0);
        if (consumed < 0) {
            return mc_decoder_fail(decoder, "av_parser_parse2", consumed);
        }

        data += consumed;
        size -= (size_t)consumed;
        if (packet_size > 0 && !mc_decoder_send_packet(decoder, packet_data, packet_size)) {
            return false;
        }

        if (consumed == 0) {
            ++no_progress_count;
            if (packet_size == 0 || no_progress_count > 4) {
                snprintf(decoder->error,
                         sizeof(decoder->error),
                         "%s parser made no progress",
                         avcodec_get_name(decoder->codec->codec_id));
                return false;
            }
        } else {
            no_progress_count = 0;
        }
    }
    return true;
}

bool mc_decoder_flush(struct mc_decoder* decoder) {
    if (!decoder->parser) {
        if (decoder->av1_tu_start != SIZE_MAX && decoder->av1_tu_start + 2 < decoder->av1_scan) {
            size_t tu_size = decoder->av1_scan - decoder->av1_tu_start;
            if (tu_size > INT_MAX ||
                !mc_decoder_send_packet(
                    decoder, decoder->av1_buffer + decoder->av1_tu_start, (int)tu_size)) {
                return false;
            }
            decoder->av1_size = 0;
            decoder->av1_scan = 0;
            decoder->av1_tu_start = SIZE_MAX;
        }
    } else {
        uint8_t* packet_data = NULL;
        int packet_size = 0;
        int result = av_parser_parse2(decoder->parser,
                                      decoder->codec,
                                      &packet_data,
                                      &packet_size,
                                      NULL,
                                      0,
                                      AV_NOPTS_VALUE,
                                      AV_NOPTS_VALUE,
                                      0);
        if (result < 0) {
            return mc_decoder_fail(decoder, "av_parser_parse2(flush)", result);
        }
        if (packet_size > 0 && !mc_decoder_send_packet(decoder, packet_data, packet_size)) {
            return false;
        }
    }

    int result = avcodec_send_packet(decoder->codec, NULL);
    if (result < 0 && result != AVERROR_EOF) {
        mc_decoder_fail(decoder, "avcodec_send_packet(flush)", result);
    } else if (mc_decoder_drain(decoder)) {
        if (!decoder->hardware_active || decoder->hardware_confirmed ||
            decoder->replay_count == 0) {
            return true;
        }
        snprintf(decoder->error, sizeof(decoder->error), "hardware decoder produced no frame");
    }

    if (decoder->hardware_active && !decoder->hardware_confirmed &&
        mc_decoder_fallback_to_software(decoder)) {
        result = avcodec_send_packet(decoder->codec, NULL);
        if (result < 0 && result != AVERROR_EOF) {
            return mc_decoder_fail(decoder, "avcodec_send_packet(flush)", result);
        }
        return mc_decoder_drain(decoder);
    }
    return false;
}

const char* mc_decoder_error(const struct mc_decoder* decoder) {
    return decoder->error[0] ? decoder->error : "unknown decoder error";
}

void mc_decoder_destroy(struct mc_decoder* decoder) {
    mc_decoder_close_codec(decoder);
    mc_decoder_clear_replay_packets(decoder);
    if (decoder->parser) {
        av_parser_close(decoder->parser);
        decoder->parser = NULL;
    }
    av_freep(&decoder->av1_buffer);
    decoder->av1_size = 0;
    decoder->av1_capacity = 0;
    decoder->av1_scan = 0;
    decoder->av1_tu_start = SIZE_MAX;
}
