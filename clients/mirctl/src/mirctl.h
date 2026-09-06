// SPDX-License-Identifier: LGPL-2.1-or-later
// Copyright (C) 2026 yearsyan and contributors

#ifndef MIRCTL_H
#define MIRCTL_H

#include <stdbool.h>
#include <stdint.h>

enum mc_stream_codec { MC_STREAM_CODEC_H264, MC_STREAM_CODEC_HEVC, MC_STREAM_CODEC_AV1 };

struct mc_options {
    const char* host;
    uint16_t port;
    bool control;
    enum mc_stream_codec codec;
};

int mc_run(const struct mc_options* options);

#endif
