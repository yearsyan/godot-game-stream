// SPDX-License-Identifier: LGPL-2.1-or-later
// Copyright (C) 2026 yearsyan and contributors

#ifndef MIRCTL_TRANSPORT_H
#define MIRCTL_TRANSPORT_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

enum mc_io_result {
    MC_IO_OK,
    MC_IO_EOF,
    MC_IO_ERROR,
};

struct mc_transport_ops {
    /* Backends are full-duplex: read() and write_all() may run concurrently.
     * interrupt() must unblock an in-flight read before destroy() returns.
     */
    enum mc_io_result (*read)(void* userdata, uint8_t* buffer, size_t capacity, size_t* received);
    bool (*write_all)(void* userdata, const uint8_t* data, size_t size);
    void (*interrupt)(void* userdata);
    const char* (*error)(const void* userdata);
    void (*destroy)(void* userdata);
};

struct mc_transport {
    const struct mc_transport_ops* ops;
    void* userdata;
};

void mc_transport_init(struct mc_transport* transport,
                       const struct mc_transport_ops* ops,
                       void* userdata);

bool mc_transport_connect_tcp(struct mc_transport* transport, const char* host, uint16_t port);

enum mc_io_result mc_transport_read(struct mc_transport* transport,
                                    uint8_t* buffer,
                                    size_t capacity,
                                    size_t* received);

bool mc_transport_write_all(struct mc_transport* transport, const uint8_t* data, size_t size);

void mc_transport_interrupt(struct mc_transport* transport);

const char* mc_transport_error(const struct mc_transport* transport);

void mc_transport_destroy(struct mc_transport* transport);

#endif
