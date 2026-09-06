// SPDX-License-Identifier: LGPL-2.1-or-later
// Copyright (C) 2026 yearsyan and contributors

#ifndef MIRCTL_RENDERER_H
#define MIRCTL_RENDERER_H

#include <SDL3/SDL.h>

#include <libavutil/frame.h>
#include <stdbool.h>

struct mc_renderer {
    SDL_Window* window;
    SDL_Renderer* renderer;
    SDL_Texture* texture;
    int video_width;
    int video_height;
    int video_format;
};

bool mc_renderer_init(struct mc_renderer* renderer);

bool mc_renderer_present_frame(struct mc_renderer* renderer, const AVFrame* frame);

bool mc_renderer_redraw(struct mc_renderer* renderer);

bool mc_renderer_convert_mouse_event(const struct mc_renderer* renderer, SDL_Event* event);

bool mc_renderer_has_video(const struct mc_renderer* renderer);

void mc_renderer_destroy(struct mc_renderer* renderer);

#endif
