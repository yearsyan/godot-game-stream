// SPDX-License-Identifier: LGPL-2.1-or-later
// Copyright (C) 2026 yearsyan and contributors

#include "mirctl.h"

#include <errno.h>
#include <libavcodec/avcodec.h>
#include <libavutil/avutil.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

enum { MC_DEFAULT_PORT = 20831, MC_HOST_CAPACITY = 256 };

static void mc_usage(const char* program) {
    printf("Usage: %s [--no-control] [--codec=h264|hevc|av1] [HOST[:PORT]]\n", program);
    printf("       %s [--no-control] [--codec=h264|hevc|av1] [[IPv6]:PORT]\n", program);
    printf("Default endpoint: 127.0.0.1:%d, default codec: h264\n", MC_DEFAULT_PORT);
    printf("Use --version to show the version and FFmpeg license.\n");
}

static bool mc_parse_port(const char* text, uint16_t* port) {
    if (!text || !*text) {
        return false;
    }
    errno = 0;
    char* end = NULL;
    long value = strtol(text, &end, 10);
    if (errno || !end || *end || value < 1 || value > 65535) {
        return false;
    }
    *port = (uint16_t)value;
    return true;
}

static bool mc_copy_host(char* host, size_t capacity, const char* begin, size_t length) {
    if (length == 0 || length >= capacity) {
        return false;
    }
    memcpy(host, begin, length);
    host[length] = '\0';
    return true;
}

static bool
mc_parse_endpoint(const char* endpoint, char* host, size_t host_capacity, uint16_t* port) {
    *port = MC_DEFAULT_PORT;
    if (endpoint[0] == '[') {
        const char* closing = strchr(endpoint + 1, ']');
        if (!closing ||
            !mc_copy_host(host, host_capacity, endpoint + 1, (size_t)(closing - endpoint - 1))) {
            return false;
        }
        if (!closing[1]) {
            return true;
        }
        return closing[1] == ':' && mc_parse_port(closing + 2, port);
    }

    const char* first_colon = strchr(endpoint, ':');
    const char* last_colon = strrchr(endpoint, ':');
    if (first_colon && first_colon == last_colon) {
        if (!mc_copy_host(host, host_capacity, endpoint, (size_t)(first_colon - endpoint))) {
            return false;
        }
        return mc_parse_port(first_colon + 1, port);
    }

    return mc_copy_host(host, host_capacity, endpoint, strlen(endpoint));
}

static bool mc_parse_codec(const char* text, enum mc_stream_codec* codec) {
    if (!strcmp(text, "h264")) {
        *codec = MC_STREAM_CODEC_H264;
        return true;
    }
    if (!strcmp(text, "hevc") || !strcmp(text, "h265")) {
        *codec = MC_STREAM_CODEC_HEVC;
        return true;
    }
    if (!strcmp(text, "av1")) {
        *codec = MC_STREAM_CODEC_AV1;
        return true;
    }
    return false;
}

int main(int argc, char** argv) {
    char host[MC_HOST_CAPACITY] = "127.0.0.1";
    uint16_t port = MC_DEFAULT_PORT;
    bool control = true;
    enum mc_stream_codec codec = MC_STREAM_CODEC_H264;
    bool endpoint_seen = false;

    for (int index = 1; index < argc; ++index) {
        const char* argument = argv[index];
        if (!strcmp(argument, "--version")) {
            printf("mirctl 0.1.0\nFFmpeg %s\nFFmpeg license: %s\n",
                   av_version_info(),
                   avcodec_license());
            return 0;
        }
        if (!strcmp(argument, "--help") || !strcmp(argument, "-h")) {
            mc_usage(argv[0]);
            return 0;
        }
        if (!strcmp(argument, "--no-control")) {
            control = false;
            continue;
        }
        if (!strncmp(argument, "--codec=", 8)) {
            if (!mc_parse_codec(argument + 8, &codec)) {
                fprintf(stderr, "invalid codec: %s\n", argument + 8);
                mc_usage(argv[0]);
                return 2;
            }
            continue;
        }
        if (argument[0] == '-' || endpoint_seen ||
            !mc_parse_endpoint(argument, host, sizeof(host), &port)) {
            fprintf(stderr, "invalid argument or endpoint: %s\n", argument);
            mc_usage(argv[0]);
            return 2;
        }
        endpoint_seen = true;
    }

    struct mc_options options = {
        .host = host,
        .port = port,
        .control = control,
        .codec = codec,
    };
    return mc_run(&options);
}
