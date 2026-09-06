// SPDX-License-Identifier: LGPL-2.1-or-later
// Copyright (C) 2026 yearsyan and contributors

#ifndef MIRCTL_CONTROL_H
#define MIRCTL_CONTROL_H

#include "transport.h"

#include <SDL3/SDL.h>

#include <stdbool.h>
#include <stdint.h>

struct mc_controller {
    struct mc_transport* transport;
};

void mc_controller_init(struct mc_controller* controller, struct mc_transport* transport);

bool mc_controller_send_key(struct mc_controller* controller, const SDL_KeyboardEvent* event);

bool mc_controller_send_mouse_motion(
    struct mc_controller* controller, float x, float y, float dx, float dy);

bool mc_controller_send_mouse_button(struct mc_controller* controller,
                                     uint8_t sdl_button,
                                     bool pressed,
                                     uint8_t clicks,
                                     float x,
                                     float y);

bool mc_controller_send_mouse_wheel(
    struct mc_controller* controller, float x, float y, float wheel_x, float wheel_y);

#endif
