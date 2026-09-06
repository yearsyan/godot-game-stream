// SPDX-License-Identifier: LGPL-2.1-or-later
// Copyright (C) 2026 yearsyan and contributors

#include "control.h"
#include "decoder.h"
#include "mirctl.h"
#include "renderer.h"
#include "transport.h"

#include <SDL3/SDL.h>

#include <libavutil/log.h>
#include <libavutil/mem.h>
#include <stdatomic.h>
#include <stdio.h>
#include <string.h>

enum {
    MC_EVENT_NEW_FRAME = SDL_EVENT_USER,
    MC_EVENT_STREAM_ENDED,
    MC_READ_BUFFER_SIZE = 64 * 1024,
};

struct mc_frame_mailbox {
    SDL_Mutex* mutex;
    AVFrame* latest;
    bool event_pending;
};

struct mc_app {
    struct mc_transport transport;
    struct mc_decoder decoder;
    struct mc_renderer renderer;
    struct mc_controller controller;
    struct mc_frame_mailbox mailbox;
    SDL_Thread* worker;
    atomic_bool stopping;
    atomic_bool worker_done;
    atomic_int worker_result;
};

static bool mc_mailbox_init(struct mc_frame_mailbox* mailbox) {
    memset(mailbox, 0, sizeof(*mailbox));
    mailbox->mutex = SDL_CreateMutex();
    mailbox->latest = av_frame_alloc();
    if (!mailbox->mutex || !mailbox->latest) {
        fprintf(stderr, "could not allocate video frame mailbox\n");
        av_frame_free(&mailbox->latest);
        if (mailbox->mutex) {
            SDL_DestroyMutex(mailbox->mutex);
            mailbox->mutex = NULL;
        }
        return false;
    }
    return true;
}

static void mc_mailbox_destroy(struct mc_frame_mailbox* mailbox) {
    av_frame_free(&mailbox->latest);
    if (mailbox->mutex) {
        SDL_DestroyMutex(mailbox->mutex);
    }
    memset(mailbox, 0, sizeof(*mailbox));
}

static bool mc_mailbox_push(struct mc_frame_mailbox* mailbox, const AVFrame* frame) {
    SDL_LockMutex(mailbox->mutex);
    av_frame_unref(mailbox->latest);
    int result = av_frame_ref(mailbox->latest, frame);
    bool post_event = result >= 0 && !mailbox->event_pending;
    if (post_event) {
        mailbox->event_pending = true;
    }
    SDL_UnlockMutex(mailbox->mutex);

    if (result < 0) {
        return false;
    }
    if (!post_event) {
        return true;
    }

    SDL_Event event;
    memset(&event, 0, sizeof(event));
    event.type = MC_EVENT_NEW_FRAME;
    if (SDL_PushEvent(&event)) {
        return true;
    }

    SDL_LockMutex(mailbox->mutex);
    mailbox->event_pending = false;
    SDL_UnlockMutex(mailbox->mutex);
    fprintf(stderr, "SDL_PushEvent failed: %s\n", SDL_GetError());
    return false;
}

static bool mc_mailbox_take(struct mc_frame_mailbox* mailbox, AVFrame* destination) {
    SDL_LockMutex(mailbox->mutex);
    bool available = mailbox->latest->width > 0;
    if (available) {
        av_frame_unref(destination);
        av_frame_move_ref(destination, mailbox->latest);
    }
    mailbox->event_pending = false;
    SDL_UnlockMutex(mailbox->mutex);
    return available;
}

static bool mc_on_decoded_frame(void* userdata, const AVFrame* frame) {
    struct mc_app* app = userdata;
    if (atomic_load_explicit(&app->stopping, memory_order_acquire)) {
        return false;
    }
    return mc_mailbox_push(&app->mailbox, frame);
}

static int SDLCALL mc_video_worker(void* userdata) {
    struct mc_app* app = userdata;
    uint8_t* buffer = av_malloc(MC_READ_BUFFER_SIZE + AV_INPUT_BUFFER_PADDING_SIZE);
    int status = 0;

    if (!buffer) {
        fprintf(stderr, "could not allocate stream receive buffer\n");
        status = 1;
    }

    while (buffer && !atomic_load_explicit(&app->stopping, memory_order_acquire)) {
        size_t received = 0;
        enum mc_io_result io =
            mc_transport_read(&app->transport, buffer, MC_READ_BUFFER_SIZE, &received);
        if (io == MC_IO_EOF) {
            break;
        }
        if (io == MC_IO_ERROR) {
            if (!atomic_load_explicit(&app->stopping, memory_order_acquire)) {
                fprintf(stderr, "stream read failed: %s\n", mc_transport_error(&app->transport));
                status = 1;
            }
            break;
        }

        memset(buffer + received, 0, AV_INPUT_BUFFER_PADDING_SIZE);
        if (!mc_decoder_push(&app->decoder, buffer, received)) {
            if (!atomic_load_explicit(&app->stopping, memory_order_acquire)) {
                fprintf(stderr, "decode failed: %s\n", mc_decoder_error(&app->decoder));
                status = 1;
            }
            break;
        }
    }

    if (buffer && status == 0 && !atomic_load_explicit(&app->stopping, memory_order_acquire) &&
        !mc_decoder_flush(&app->decoder)) {
        fprintf(stderr, "decoder flush failed: %s\n", mc_decoder_error(&app->decoder));
        status = 1;
    }
    av_free(buffer);

    atomic_store_explicit(&app->worker_result, status, memory_order_release);
    atomic_store_explicit(&app->worker_done, true, memory_order_release);

    SDL_Event event;
    memset(&event, 0, sizeof(event));
    event.type = MC_EVENT_STREAM_ENDED;
    SDL_PushEvent(&event);
    return status;
}

static bool mc_send_input(
    struct mc_app* app, const SDL_Event* event, float* last_x, float* last_y, bool* has_pointer) {
    switch (event->type) {
    case SDL_EVENT_KEY_DOWN:
    case SDL_EVENT_KEY_UP:
        return mc_controller_send_key(&app->controller, &event->key);

    case SDL_EVENT_MOUSE_MOTION: {
        SDL_Event converted = *event;
        if (!mc_renderer_convert_mouse_event(&app->renderer, &converted)) {
            return true;
        }
        *last_x = converted.motion.x;
        *last_y = converted.motion.y;
        *has_pointer = true;
        return mc_controller_send_mouse_motion(&app->controller,
                                               converted.motion.x,
                                               converted.motion.y,
                                               converted.motion.xrel,
                                               converted.motion.yrel);
    }

    case SDL_EVENT_MOUSE_BUTTON_DOWN:
    case SDL_EVENT_MOUSE_BUTTON_UP: {
        SDL_Event converted = *event;
        bool inside = mc_renderer_convert_mouse_event(&app->renderer, &converted);
        bool pressed = event->type == SDL_EVENT_MOUSE_BUTTON_DOWN;
        if (!inside && (pressed || !*has_pointer)) {
            return true;
        }
        float x = inside ? converted.button.x : *last_x;
        float y = inside ? converted.button.y : *last_y;
        if (inside) {
            *last_x = x;
            *last_y = y;
            *has_pointer = true;
        }
        return mc_controller_send_mouse_button(
            &app->controller, event->button.button, pressed, event->button.clicks, x, y);
    }

    case SDL_EVENT_MOUSE_WHEEL: {
        SDL_Event converted = *event;
        if (!mc_renderer_convert_mouse_event(&app->renderer, &converted)) {
            return true;
        }
        *last_x = converted.wheel.mouse_x;
        *last_y = converted.wheel.mouse_y;
        *has_pointer = true;
        float direction = event->wheel.direction == SDL_MOUSEWHEEL_FLIPPED ? -1.0f : 1.0f;
        return mc_controller_send_mouse_wheel(&app->controller,
                                              *last_x,
                                              *last_y,
                                              event->wheel.x * direction,
                                              event->wheel.y * direction);
    }

    default:
        return true;
    }
}

static int mc_event_loop(struct mc_app* app, bool control_enabled) {
    AVFrame* frame = av_frame_alloc();
    if (!frame) {
        fprintf(stderr, "could not allocate display frame\n");
        return 1;
    }

    bool running = true;
    bool user_quit = false;
    bool has_pointer = false;
    float last_x = 0.0f;
    float last_y = 0.0f;
    int result = 0;

    while (running) {
        SDL_Event event;
        bool has_event = SDL_WaitEventTimeout(&event, 100);
        while (has_event) {
            switch (event.type) {
            case SDL_EVENT_QUIT:
            case SDL_EVENT_WINDOW_CLOSE_REQUESTED:
                user_quit = true;
                running = false;
                break;

            case SDL_EVENT_WINDOW_EXPOSED:
            case SDL_EVENT_WINDOW_RESIZED:
            case SDL_EVENT_WINDOW_PIXEL_SIZE_CHANGED:
                if (!mc_renderer_redraw(&app->renderer)) {
                    result = 1;
                    running = false;
                }
                break;

            case SDL_EVENT_KEY_DOWN:
            case SDL_EVENT_KEY_UP:
            case SDL_EVENT_MOUSE_MOTION:
            case SDL_EVENT_MOUSE_BUTTON_DOWN:
            case SDL_EVENT_MOUSE_BUTTON_UP:
            case SDL_EVENT_MOUSE_WHEEL:
                if (control_enabled &&
                    !mc_send_input(app, &event, &last_x, &last_y, &has_pointer)) {
                    fprintf(
                        stderr, "control send failed: %s\n", mc_transport_error(&app->transport));
                    result = 1;
                    running = false;
                }
                break;

            default:
                break;
            }

            if (!running || !SDL_PollEvent(&event)) {
                break;
            }
        }

        if (mc_mailbox_take(&app->mailbox, frame) &&
            !mc_renderer_present_frame(&app->renderer, frame)) {
            result = 1;
            running = false;
        }

        if (atomic_load_explicit(&app->worker_done, memory_order_acquire)) {
            running = false;
        }
    }

    if (!user_quit) {
        int worker_result = atomic_load_explicit(&app->worker_result, memory_order_acquire);
        if (worker_result != 0) {
            result = worker_result;
        }
    }
    av_frame_free(&frame);
    return result;
}

int mc_run(const struct mc_options* options) {
    struct mc_app app;
    memset(&app, 0, sizeof(app));
    atomic_init(&app.stopping, false);
    atomic_init(&app.worker_done, false);
    atomic_init(&app.worker_result, 0);

    av_log_set_level(AV_LOG_WARNING);
    if (!SDL_Init(SDL_INIT_VIDEO)) {
        fprintf(stderr, "SDL_Init failed: %s\n", SDL_GetError());
        return 1;
    }

    int result = 1;
    bool renderer_initialized = false;
    bool mailbox_initialized = false;
    bool decoder_initialized = false;

    if (!mc_renderer_init(&app.renderer)) {
        goto end;
    }
    renderer_initialized = true;

    if (!mc_mailbox_init(&app.mailbox)) {
        goto end;
    }
    mailbox_initialized = true;

    enum AVCodecID codec_id = AV_CODEC_ID_H264;
    if (options->codec == MC_STREAM_CODEC_AV1) {
        codec_id = AV_CODEC_ID_AV1;
    } else if (options->codec == MC_STREAM_CODEC_HEVC) {
        codec_id = AV_CODEC_ID_HEVC;
    }

    fprintf(stderr, "connecting to %s:%u...\n", options->host, (unsigned)options->port);
    if (!mc_transport_connect_tcp(&app.transport, options->host, options->port)) {
        fprintf(stderr, "connect failed: %s\n", mc_transport_error(&app.transport));
        goto end;
    }
    fprintf(stderr,
            "connected; waiting for %s stream%s\n",
            avcodec_get_name(codec_id),
            options->control ? ", control enabled" : "");

    if (!mc_decoder_init(&app.decoder, codec_id, mc_on_decoded_frame, &app)) {
        fprintf(stderr, "decoder init failed: %s\n", mc_decoder_error(&app.decoder));
        goto end;
    }
    decoder_initialized = true;
    mc_controller_init(&app.controller, &app.transport);

    app.worker = SDL_CreateThread(mc_video_worker, "mirctl-video", &app);
    if (!app.worker) {
        fprintf(stderr, "SDL_CreateThread failed: %s\n", SDL_GetError());
        goto end;
    }

    result = mc_event_loop(&app, options->control);

end:
    atomic_store_explicit(&app.stopping, true, memory_order_release);
    mc_transport_interrupt(&app.transport);
    if (app.worker) {
        SDL_WaitThread(app.worker, NULL);
    }
    if (decoder_initialized) {
        mc_decoder_destroy(&app.decoder);
    }
    mc_transport_destroy(&app.transport);
    if (mailbox_initialized) {
        mc_mailbox_destroy(&app.mailbox);
    }
    if (renderer_initialized) {
        mc_renderer_destroy(&app.renderer);
    }
    SDL_Quit();
    return result;
}
