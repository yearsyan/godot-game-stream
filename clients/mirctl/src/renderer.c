// SPDX-License-Identifier: LGPL-2.1-or-later
// Copyright (C) 2026 yearsyan and contributors

#include "renderer.h"

#include <libavutil/pixdesc.h>
#include <libavutil/pixfmt.h>
#include <stdio.h>
#include <string.h>

static const char* mc_pixel_format_name(enum AVPixelFormat format) {
    const char* name = av_get_pix_fmt_name(format);
    return name ? name : "unknown";
}

static bool mc_pixel_format_is_semiplanar(enum AVPixelFormat format) {
    return format == AV_PIX_FMT_NV12 || format == AV_PIX_FMT_P010;
}

static SDL_Colorspace mc_renderer_colorspace(const AVFrame* frame) {
    bool full_range = frame->color_range == AVCOL_RANGE_JPEG;
    switch (frame->colorspace) {
    case AVCOL_SPC_BT470BG:
    case AVCOL_SPC_SMPTE170M:
        return full_range ? SDL_COLORSPACE_BT601_FULL : SDL_COLORSPACE_BT601_LIMITED;
    case AVCOL_SPC_BT2020_NCL:
    case AVCOL_SPC_BT2020_CL:
        return full_range ? SDL_COLORSPACE_BT2020_FULL : SDL_COLORSPACE_BT2020_LIMITED;
    case AVCOL_SPC_BT709:
    case AVCOL_SPC_RGB:
    case AVCOL_SPC_UNSPECIFIED:
    default:
        return full_range ? SDL_COLORSPACE_BT709_FULL : SDL_COLORSPACE_BT709_LIMITED;
    }
}

static void mc_renderer_resize_window_to_video(struct mc_renderer* renderer,
                                               int video_width,
                                               int video_height) {
    float reported_density = SDL_GetWindowPixelDensity(renderer->window);
    double pixel_density = reported_density > 0.0f ? reported_density : 1.0;
    double fit_scale = 1.0;
    SDL_DisplayID display = SDL_GetDisplayForWindow(renderer->window);
    SDL_Rect usable_bounds = {0};
    bool have_usable_bounds = display != 0 && SDL_GetDisplayUsableBounds(display, &usable_bounds);

    if (have_usable_bounds) {
        int top = 0;
        int left = 0;
        int bottom = 0;
        int right = 0;
        if (!SDL_GetWindowBordersSize(renderer->window, &top, &left, &bottom, &right)) {
            top = 0;
            left = 0;
            bottom = 0;
            right = 0;
        }

        int max_width = usable_bounds.w - left - right;
        int max_height = usable_bounds.h - top - bottom;
        if (max_width > 0 && max_height > 0) {
            double width_scale = ((double)max_width * pixel_density) / video_width;
            double height_scale = ((double)max_height * pixel_density) / video_height;
            if (width_scale < fit_scale) {
                fit_scale = width_scale;
            }
            if (height_scale < fit_scale) {
                fit_scale = height_scale;
            }
        }
    }

    int window_width = (int)(((double)video_width * fit_scale) / pixel_density);
    int window_height = (int)(((double)video_height * fit_scale) / pixel_density);
    if (window_width < 1) {
        window_width = 1;
    }
    if (window_height < 1) {
        window_height = 1;
    }

    if (!SDL_SetWindowSize(renderer->window, window_width, window_height)) {
        fprintf(stderr, "SDL_SetWindowSize failed: %s\n", SDL_GetError());
        return;
    }
    if (display != 0) {
        SDL_SetWindowPosition(renderer->window,
                              SDL_WINDOWPOS_CENTERED_DISPLAY(display),
                              SDL_WINDOWPOS_CENTERED_DISPLAY(display));
    }

    fprintf(stderr,
            "window: %dx%d coordinates for %dx%d video at %.2fx pixel density (%s)\n",
            window_width,
            window_height,
            video_width,
            video_height,
            pixel_density,
            fit_scale < 1.0 ? "fit-to-display" : "native pixels");
}

static bool mc_renderer_create_texture(struct mc_renderer* renderer, const AVFrame* frame) {
    if (renderer->texture) {
        SDL_DestroyTexture(renderer->texture);
        renderer->texture = NULL;
    }

    SDL_PropertiesID properties = SDL_CreateProperties();
    if (!properties) {
        fprintf(stderr, "SDL_CreateProperties failed: %s\n", SDL_GetError());
        return false;
    }

    Uint32 pixel_format;
    if (frame->format == AV_PIX_FMT_NV12) {
        pixel_format = SDL_PIXELFORMAT_NV12;
    } else if (frame->format == AV_PIX_FMT_P010) {
        pixel_format = SDL_PIXELFORMAT_P010;
    } else {
        pixel_format = SDL_PIXELFORMAT_IYUV;
    }
    bool ok =
        SDL_SetNumberProperty(properties, SDL_PROP_TEXTURE_CREATE_FORMAT_NUMBER, pixel_format);
    ok &= SDL_SetNumberProperty(
        properties, SDL_PROP_TEXTURE_CREATE_ACCESS_NUMBER, SDL_TEXTUREACCESS_STREAMING);
    ok &= SDL_SetNumberProperty(properties, SDL_PROP_TEXTURE_CREATE_WIDTH_NUMBER, frame->width);
    ok &= SDL_SetNumberProperty(properties, SDL_PROP_TEXTURE_CREATE_HEIGHT_NUMBER, frame->height);
    ok &= SDL_SetNumberProperty(
        properties, SDL_PROP_TEXTURE_CREATE_COLORSPACE_NUMBER, mc_renderer_colorspace(frame));
    if (!ok) {
        fprintf(stderr, "could not configure SDL video texture: %s\n", SDL_GetError());
        SDL_DestroyProperties(properties);
        return false;
    }

    renderer->texture = SDL_CreateTextureWithProperties(renderer->renderer, properties);
    SDL_DestroyProperties(properties);
    if (!renderer->texture) {
        fprintf(stderr, "SDL_CreateTextureWithProperties failed: %s\n", SDL_GetError());
        return false;
    }

    SDL_SetTextureScaleMode(renderer->texture, SDL_SCALEMODE_LINEAR);
    if (!SDL_SetRenderLogicalPresentation(
            renderer->renderer, frame->width, frame->height, SDL_LOGICAL_PRESENTATION_LETTERBOX)) {
        fprintf(stderr, "SDL_SetRenderLogicalPresentation failed: %s\n", SDL_GetError());
        SDL_DestroyTexture(renderer->texture);
        renderer->texture = NULL;
        return false;
    }
    mc_renderer_resize_window_to_video(renderer, frame->width, frame->height);

    renderer->video_width = frame->width;
    renderer->video_height = frame->height;
    renderer->video_format = frame->format;
    char title[96];
    snprintf(title, sizeof(title), "mirctl - %dx%d", frame->width, frame->height);
    SDL_SetWindowTitle(renderer->window, title);
    fprintf(stderr,
            "video: %dx%d, pixel format=%s\n",
            frame->width,
            frame->height,
            mc_pixel_format_name((enum AVPixelFormat)frame->format));
    return true;
}

bool mc_renderer_init(struct mc_renderer* renderer) {
    memset(renderer, 0, sizeof(*renderer));
    renderer->window = SDL_CreateWindow(
        "mirctl - connecting", 1280, 720, SDL_WINDOW_RESIZABLE | SDL_WINDOW_HIGH_PIXEL_DENSITY);
    if (!renderer->window) {
        fprintf(stderr, "SDL_CreateWindow failed: %s\n", SDL_GetError());
        return false;
    }

    renderer->renderer = SDL_CreateRenderer(renderer->window, NULL);
    if (!renderer->renderer) {
        fprintf(stderr, "SDL_CreateRenderer failed: %s\n", SDL_GetError());
        mc_renderer_destroy(renderer);
        return false;
    }

    SDL_SetRenderVSync(renderer->renderer, 0);
    SDL_SetRenderDrawColor(renderer->renderer, 0, 0, 0, 255);
    const char* name = SDL_GetRendererName(renderer->renderer);
    fprintf(stderr, "SDL renderer: %s\n", name ? name : "unknown");
    return mc_renderer_redraw(renderer);
}

bool mc_renderer_present_frame(struct mc_renderer* renderer, const AVFrame* frame) {
    if (frame->format != AV_PIX_FMT_YUV420P && frame->format != AV_PIX_FMT_YUVJ420P &&
        !mc_pixel_format_is_semiplanar((enum AVPixelFormat)frame->format)) {
        fprintf(stderr,
                "unsupported decoded pixel format: %s\n",
                mc_pixel_format_name((enum AVPixelFormat)frame->format));
        return false;
    }

    if (!renderer->texture || renderer->video_width != frame->width ||
        renderer->video_height != frame->height || renderer->video_format != frame->format) {
        if (!mc_renderer_create_texture(renderer, frame)) {
            return false;
        }
    }

    bool semiplanar = mc_pixel_format_is_semiplanar((enum AVPixelFormat)frame->format);
    bool uploaded = semiplanar ? SDL_UpdateNVTexture(renderer->texture,
                                                     NULL,
                                                     frame->data[0],
                                                     frame->linesize[0],
                                                     frame->data[1],
                                                     frame->linesize[1])
                               : SDL_UpdateYUVTexture(renderer->texture,
                                                      NULL,
                                                      frame->data[0],
                                                      frame->linesize[0],
                                                      frame->data[1],
                                                      frame->linesize[1],
                                                      frame->data[2],
                                                      frame->linesize[2]);
    if (!uploaded) {
        fprintf(stderr,
                "%s failed: %s\n",
                semiplanar ? "SDL_UpdateNVTexture" : "SDL_UpdateYUVTexture",
                SDL_GetError());
        return false;
    }
    return mc_renderer_redraw(renderer);
}

bool mc_renderer_redraw(struct mc_renderer* renderer) {
    if (!SDL_RenderClear(renderer->renderer)) {
        fprintf(stderr, "SDL_RenderClear failed: %s\n", SDL_GetError());
        return false;
    }
    if (renderer->texture &&
        !SDL_RenderTexture(renderer->renderer, renderer->texture, NULL, NULL)) {
        fprintf(stderr, "SDL_RenderTexture failed: %s\n", SDL_GetError());
        return false;
    }
    if (!SDL_RenderPresent(renderer->renderer)) {
        fprintf(stderr, "SDL_RenderPresent failed: %s\n", SDL_GetError());
        return false;
    }
    return true;
}

bool mc_renderer_convert_mouse_event(const struct mc_renderer* renderer, SDL_Event* event) {
    if (!mc_renderer_has_video(renderer) ||
        !SDL_ConvertEventToRenderCoordinates(renderer->renderer, event)) {
        return false;
    }

    float x;
    float y;
    switch (event->type) {
    case SDL_EVENT_MOUSE_MOTION:
        x = event->motion.x;
        y = event->motion.y;
        break;
    case SDL_EVENT_MOUSE_BUTTON_DOWN:
    case SDL_EVENT_MOUSE_BUTTON_UP:
        x = event->button.x;
        y = event->button.y;
        break;
    case SDL_EVENT_MOUSE_WHEEL:
        x = event->wheel.mouse_x;
        y = event->wheel.mouse_y;
        break;
    default:
        return false;
    }
    return x >= 0.0f && y >= 0.0f && x < renderer->video_width && y < renderer->video_height;
}

bool mc_renderer_has_video(const struct mc_renderer* renderer) {
    return renderer->texture && renderer->video_width > 0 && renderer->video_height > 0;
}

void mc_renderer_destroy(struct mc_renderer* renderer) {
    if (renderer->texture) {
        SDL_DestroyTexture(renderer->texture);
    }
    if (renderer->renderer) {
        SDL_DestroyRenderer(renderer->renderer);
    }
    if (renderer->window) {
        SDL_DestroyWindow(renderer->window);
    }
    memset(renderer, 0, sizeof(*renderer));
}
