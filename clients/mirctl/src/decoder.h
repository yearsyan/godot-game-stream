// SPDX-License-Identifier: LGPL-2.1-or-later
// Copyright (C) 2026 yearsyan and contributors

#ifndef MIRCTL_DECODER_H
#define MIRCTL_DECODER_H

#include <libavcodec/avcodec.h>
#include <libavutil/buffer.h>
#include <libavutil/error.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

typedef bool (*mc_decoder_frame_callback)(void* userdata, const AVFrame* frame);

struct mc_decoder {
    AVCodecParserContext* parser;
    AVCodecContext* codec;
    const AVCodec* software_codec;
    AVBufferRef* hw_device;
    AVFrame* frame;
    AVFrame* sw_frame;
    enum AVPixelFormat hw_pix_fmt;
    bool hardware_active;
    bool hardware_confirmed;
    AVPacket* replay_packets;
    size_t replay_count;
    size_t replay_capacity;
    mc_decoder_frame_callback on_frame;
    void* callback_userdata;
    /* AV1 temporal-unit framing: FFmpeg's AV1 parser is pass-through and
       performs no frame splitting, so complete temporal units are cut here
       at temporal-delimiter OBU boundaries before avcodec_send_packet. */
    uint8_t* av1_buffer;
    size_t av1_size;
    size_t av1_capacity;
    size_t av1_scan;
    size_t av1_tu_start;
    char error[AV_ERROR_MAX_STRING_SIZE];
};

bool mc_decoder_init(struct mc_decoder* decoder,
                     enum AVCodecID codec_id,
                     mc_decoder_frame_callback on_frame,
                     void* userdata);

bool mc_decoder_push(struct mc_decoder* decoder, const uint8_t* data, size_t size);

bool mc_decoder_flush(struct mc_decoder* decoder);

const char* mc_decoder_error(const struct mc_decoder* decoder);

void mc_decoder_destroy(struct mc_decoder* decoder);

#endif
