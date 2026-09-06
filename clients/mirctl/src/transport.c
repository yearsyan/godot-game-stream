// SPDX-License-Identifier: LGPL-2.1-or-later
// Copyright (C) 2026 yearsyan and contributors

#ifndef _WIN32
/* glibc hides POSIX APIs such as getaddrinfo and struct addrinfo in strict ISO C
 * mode (-std=c11). Define the feature macro before including any system header. */
#define _POSIX_C_SOURCE 200809L
#endif

#include "transport.h"

#include <errno.h>
#include <limits.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <winsock2.h>
#include <ws2tcpip.h>
typedef SOCKET mc_socket;
#define MC_INVALID_SOCKET INVALID_SOCKET
#define MC_SOCKET_ERROR SOCKET_ERROR
#else
#include <netdb.h>
#include <netinet/tcp.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <unistd.h>
typedef int mc_socket;
#define MC_INVALID_SOCKET (-1)
#define MC_SOCKET_ERROR (-1)
#endif

struct mc_tcp_transport {
    mc_socket socket;
    atomic_bool closed;
    char error[256];
#ifdef _WIN32
    bool winsock_initialized;
#endif
};

static void mc_tcp_set_error(struct mc_tcp_transport* tcp, const char* operation) {
#ifdef _WIN32
    snprintf(
        tcp->error, sizeof(tcp->error), "%s failed (WSA error %d)", operation, WSAGetLastError());
#else
    snprintf(tcp->error, sizeof(tcp->error), "%s failed: %s", operation, strerror(errno));
#endif
}

static void mc_tcp_close_socket(mc_socket socket) {
#ifdef _WIN32
    closesocket(socket);
#else
    close(socket);
#endif
}

static enum mc_io_result
mc_tcp_read(void* userdata, uint8_t* buffer, size_t capacity, size_t* received) {
    struct mc_tcp_transport* tcp = userdata;
    *received = 0;

    if (atomic_load_explicit(&tcp->closed, memory_order_acquire)) {
        return MC_IO_EOF;
    }

    for (;;) {
#ifdef _WIN32
        int request = capacity > INT_MAX ? INT_MAX : (int)capacity;
        int result = recv(tcp->socket, (char*)buffer, request, 0);
#else
        ssize_t result = recv(tcp->socket, buffer, capacity, 0);
#endif
        if (result > 0) {
            *received = (size_t)result;
            return MC_IO_OK;
        }
        if (result == 0 || atomic_load_explicit(&tcp->closed, memory_order_acquire)) {
            return MC_IO_EOF;
        }
#ifndef _WIN32
        if (errno == EINTR) {
            continue;
        }
#endif
        mc_tcp_set_error(tcp, "recv");
        return MC_IO_ERROR;
    }
}

static bool mc_tcp_write_all(void* userdata, const uint8_t* data, size_t size) {
    struct mc_tcp_transport* tcp = userdata;
    size_t written = 0;

    while (written < size) {
        if (atomic_load_explicit(&tcp->closed, memory_order_acquire)) {
            snprintf(tcp->error, sizeof(tcp->error), "transport is closed");
            return false;
        }

        size_t remaining = size - written;
#ifdef _WIN32
        int request = remaining > INT_MAX ? INT_MAX : (int)remaining;
        int result = send(tcp->socket, (const char*)data + written, request, 0);
#else
        int flags = 0;
#ifdef MSG_NOSIGNAL
        flags = MSG_NOSIGNAL;
#endif
        ssize_t result = send(tcp->socket, data + written, remaining, flags);
#endif
        if (result > 0) {
            written += (size_t)result;
            continue;
        }
#ifndef _WIN32
        if (result < 0 && errno == EINTR) {
            continue;
        }
#endif
        mc_tcp_set_error(tcp, "send");
        return false;
    }

    return true;
}

static void mc_tcp_interrupt(void* userdata) {
    struct mc_tcp_transport* tcp = userdata;
    if (atomic_exchange_explicit(&tcp->closed, true, memory_order_acq_rel)) {
        return;
    }

#ifdef _WIN32
    if (tcp->socket != MC_INVALID_SOCKET) {
        shutdown(tcp->socket, SD_BOTH);
    }
#elif defined(__APPLE__)
    if (tcp->socket != MC_INVALID_SOCKET) {
        mc_tcp_close_socket(tcp->socket);
        tcp->socket = MC_INVALID_SOCKET;
    }
#else
    if (tcp->socket != MC_INVALID_SOCKET) {
        shutdown(tcp->socket, SHUT_RDWR);
    }
#endif
}

static const char* mc_tcp_error(const void* userdata) {
    const struct mc_tcp_transport* tcp = userdata;
    return tcp->error[0] ? tcp->error : "unknown transport error";
}

static void mc_tcp_destroy(void* userdata) {
    struct mc_tcp_transport* tcp = userdata;
    atomic_store_explicit(&tcp->closed, true, memory_order_release);
    if (tcp->socket != MC_INVALID_SOCKET) {
        mc_tcp_close_socket(tcp->socket);
    }
#ifdef _WIN32
    if (tcp->winsock_initialized) {
        WSACleanup();
    }
#endif
    free(tcp);
}

static const struct mc_transport_ops MC_TCP_OPS = {
    .read = mc_tcp_read,
    .write_all = mc_tcp_write_all,
    .interrupt = mc_tcp_interrupt,
    .error = mc_tcp_error,
    .destroy = mc_tcp_destroy,
};

void mc_transport_init(struct mc_transport* transport,
                       const struct mc_transport_ops* ops,
                       void* userdata) {
    transport->ops = ops;
    transport->userdata = userdata;
}

bool mc_transport_connect_tcp(struct mc_transport* transport, const char* host, uint16_t port) {
    transport->ops = NULL;
    transport->userdata = NULL;

    struct mc_tcp_transport* tcp = calloc(1, sizeof(*tcp));
    if (!tcp) {
        return false;
    }
    tcp->socket = MC_INVALID_SOCKET;
    atomic_init(&tcp->closed, false);
    mc_transport_init(transport, &MC_TCP_OPS, tcp);

#ifdef _WIN32
    WSADATA winsock;
    int startup_result = WSAStartup(MAKEWORD(2, 2), &winsock);
    if (startup_result != 0) {
        snprintf(
            tcp->error, sizeof(tcp->error), "WSAStartup failed (WSA error %d)", startup_result);
        return false;
    }
    tcp->winsock_initialized = true;
#endif

    char service[6];
    snprintf(service, sizeof(service), "%u", (unsigned)port);
    struct addrinfo hints;
    memset(&hints, 0, sizeof(hints));
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    hints.ai_protocol = IPPROTO_TCP;

    struct addrinfo* addresses = NULL;
    int resolve_result = getaddrinfo(host, service, &hints, &addresses);
    if (resolve_result != 0) {
        snprintf(tcp->error,
                 sizeof(tcp->error),
                 "getaddrinfo(%s:%s) failed (%d)",
                 host,
                 service,
                 resolve_result);
        return false;
    }

    for (const struct addrinfo* address = addresses; address; address = address->ai_next) {
        mc_socket socket_handle =
            socket(address->ai_family, address->ai_socktype, address->ai_protocol);
        if (socket_handle == MC_INVALID_SOCKET) {
            continue;
        }

        if (connect(socket_handle,
                    address->ai_addr,
#ifdef _WIN32
                    (int)address->ai_addrlen
#else
                    address->ai_addrlen
#endif
                    ) == 0) {
            tcp->socket = socket_handle;
            break;
        }
        mc_tcp_set_error(tcp, "connect");
        mc_tcp_close_socket(socket_handle);
    }
    freeaddrinfo(addresses);

    if (tcp->socket == MC_INVALID_SOCKET) {
        if (!tcp->error[0]) {
            snprintf(tcp->error, sizeof(tcp->error), "could not connect to %s:%s", host, service);
        }
        return false;
    }

    int enabled = 1;
    if (setsockopt(tcp->socket, IPPROTO_TCP, TCP_NODELAY, (const char*)&enabled, sizeof(enabled)) ==
        MC_SOCKET_ERROR) {
        mc_tcp_set_error(tcp, "setsockopt(TCP_NODELAY)");
        return false;
    }

    return true;
}

enum mc_io_result mc_transport_read(struct mc_transport* transport,
                                    uint8_t* buffer,
                                    size_t capacity,
                                    size_t* received) {
    return transport->ops->read(transport->userdata, buffer, capacity, received);
}

bool mc_transport_write_all(struct mc_transport* transport, const uint8_t* data, size_t size) {
    return transport->ops->write_all(transport->userdata, data, size);
}

void mc_transport_interrupt(struct mc_transport* transport) {
    if (transport->ops) {
        transport->ops->interrupt(transport->userdata);
    }
}

const char* mc_transport_error(const struct mc_transport* transport) {
    if (!transport->ops) {
        return "transport is not initialized";
    }
    return transport->ops->error(transport->userdata);
}

void mc_transport_destroy(struct mc_transport* transport) {
    if (transport->ops) {
        transport->ops->destroy(transport->userdata);
        transport->ops = NULL;
        transport->userdata = NULL;
    }
}
