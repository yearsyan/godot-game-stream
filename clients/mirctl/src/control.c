// SPDX-License-Identifier: LGPL-2.1-or-later
// Copyright (C) 2026 yearsyan and contributors

#include "control.h"

#include <math.h>
#include <string.h>

enum {
    MC_GSI_HEADER_SIZE = 6,
    MC_GSI_EVENT_KEY = 1,
    MC_GSI_EVENT_MOUSE_BUTTON = 2,
    MC_GSI_EVENT_MOUSE_MOTION = 3,
};

_Static_assert(sizeof(float) == sizeof(uint32_t), "GSI1 requires 32-bit floats");

#define MC_GODOT_KEY_SPECIAL UINT32_C(4194304)

static void mc_write_u16_le(uint8_t* destination, uint16_t value) {
    destination[0] = (uint8_t)value;
    destination[1] = (uint8_t)(value >> 8);
}

static void mc_write_u32_le(uint8_t* destination, uint32_t value) {
    destination[0] = (uint8_t)value;
    destination[1] = (uint8_t)(value >> 8);
    destination[2] = (uint8_t)(value >> 16);
    destination[3] = (uint8_t)(value >> 24);
}

static void mc_write_f32_le(uint8_t* destination, float value) {
    uint32_t bits;
    memcpy(&bits, &value, sizeof(bits));
    mc_write_u32_le(destination, bits);
}

static void mc_gsi_header(uint8_t* message, uint16_t payload_size) {
    /* The current server serializes numeric 0x47534931 as little-endian, so
     * the actual four wire bytes are "1ISG". Keep this exact for compatibility.
     */
    mc_write_u32_le(message, UINT32_C(0x47534931));
    mc_write_u16_le(message + 4, payload_size);
}

static bool mc_send(struct mc_controller* controller, const uint8_t* message, size_t size) {
    return mc_transport_write_all(controller->transport, message, size);
}

static uint32_t mc_godot_key_from_sdl(SDL_Keycode key) {
    if (key >= SDLK_A && key <= SDLK_Z) {
        return (uint32_t)('A' + (key - SDLK_A));
    }
    if (key >= 32 && key <= 126) {
        return (uint32_t)key;
    }

    switch (key) {
    case SDLK_ESCAPE:
        return MC_GODOT_KEY_SPECIAL + 1;
    case SDLK_TAB:
        return MC_GODOT_KEY_SPECIAL + 2;
    case SDLK_BACKSPACE:
        return MC_GODOT_KEY_SPECIAL + 4;
    case SDLK_RETURN:
        return MC_GODOT_KEY_SPECIAL + 5;
    case SDLK_KP_ENTER:
        return MC_GODOT_KEY_SPECIAL + 6;
    case SDLK_INSERT:
        return MC_GODOT_KEY_SPECIAL + 7;
    case SDLK_DELETE:
        return MC_GODOT_KEY_SPECIAL + 8;
    case SDLK_PAUSE:
        return MC_GODOT_KEY_SPECIAL + 9;
    case SDLK_PRINTSCREEN:
        return MC_GODOT_KEY_SPECIAL + 10;
    case SDLK_HOME:
        return MC_GODOT_KEY_SPECIAL + 13;
    case SDLK_END:
        return MC_GODOT_KEY_SPECIAL + 14;
    case SDLK_LEFT:
        return MC_GODOT_KEY_SPECIAL + 15;
    case SDLK_UP:
        return MC_GODOT_KEY_SPECIAL + 16;
    case SDLK_RIGHT:
        return MC_GODOT_KEY_SPECIAL + 17;
    case SDLK_DOWN:
        return MC_GODOT_KEY_SPECIAL + 18;
    case SDLK_PAGEUP:
        return MC_GODOT_KEY_SPECIAL + 19;
    case SDLK_PAGEDOWN:
        return MC_GODOT_KEY_SPECIAL + 20;
    case SDLK_LSHIFT:
    case SDLK_RSHIFT:
        return MC_GODOT_KEY_SPECIAL + 21;
    case SDLK_LCTRL:
    case SDLK_RCTRL:
        return MC_GODOT_KEY_SPECIAL + 22;
    case SDLK_LGUI:
    case SDLK_RGUI:
        return MC_GODOT_KEY_SPECIAL + 23;
    case SDLK_LALT:
    case SDLK_RALT:
        return MC_GODOT_KEY_SPECIAL + 24;
    case SDLK_CAPSLOCK:
        return MC_GODOT_KEY_SPECIAL + 25;
    case SDLK_NUMLOCKCLEAR:
        return MC_GODOT_KEY_SPECIAL + 26;
    case SDLK_SCROLLLOCK:
        return MC_GODOT_KEY_SPECIAL + 27;
    case SDLK_F1:
        return MC_GODOT_KEY_SPECIAL + 28;
    case SDLK_F2:
        return MC_GODOT_KEY_SPECIAL + 29;
    case SDLK_F3:
        return MC_GODOT_KEY_SPECIAL + 30;
    case SDLK_F4:
        return MC_GODOT_KEY_SPECIAL + 31;
    case SDLK_F5:
        return MC_GODOT_KEY_SPECIAL + 32;
    case SDLK_F6:
        return MC_GODOT_KEY_SPECIAL + 33;
    case SDLK_F7:
        return MC_GODOT_KEY_SPECIAL + 34;
    case SDLK_F8:
        return MC_GODOT_KEY_SPECIAL + 35;
    case SDLK_F9:
        return MC_GODOT_KEY_SPECIAL + 36;
    case SDLK_F10:
        return MC_GODOT_KEY_SPECIAL + 37;
    case SDLK_F11:
        return MC_GODOT_KEY_SPECIAL + 38;
    case SDLK_F12:
        return MC_GODOT_KEY_SPECIAL + 39;
    case SDLK_KP_MULTIPLY:
        return MC_GODOT_KEY_SPECIAL + 129;
    case SDLK_KP_DIVIDE:
        return MC_GODOT_KEY_SPECIAL + 130;
    case SDLK_KP_MINUS:
        return MC_GODOT_KEY_SPECIAL + 131;
    case SDLK_KP_PERIOD:
        return MC_GODOT_KEY_SPECIAL + 132;
    case SDLK_KP_PLUS:
        return MC_GODOT_KEY_SPECIAL + 133;
    case SDLK_KP_0:
        return MC_GODOT_KEY_SPECIAL + 134;
    case SDLK_KP_1:
        return MC_GODOT_KEY_SPECIAL + 135;
    case SDLK_KP_2:
        return MC_GODOT_KEY_SPECIAL + 136;
    case SDLK_KP_3:
        return MC_GODOT_KEY_SPECIAL + 137;
    case SDLK_KP_4:
        return MC_GODOT_KEY_SPECIAL + 138;
    case SDLK_KP_5:
        return MC_GODOT_KEY_SPECIAL + 139;
    case SDLK_KP_6:
        return MC_GODOT_KEY_SPECIAL + 140;
    case SDLK_KP_7:
        return MC_GODOT_KEY_SPECIAL + 141;
    case SDLK_KP_8:
        return MC_GODOT_KEY_SPECIAL + 142;
    case SDLK_KP_9:
        return MC_GODOT_KEY_SPECIAL + 143;
    default:
        return 0;
    }
}

static uint32_t mc_unicode_from_sdl(SDL_Keycode key) {
    return key >= 32 && key <= 0x10ffff ? (uint32_t)key : 0;
}

static uint8_t mc_mouse_button_from_sdl(uint8_t button) {
    switch (button) {
    case SDL_BUTTON_LEFT:
        return 1;
    case SDL_BUTTON_RIGHT:
        return 2;
    case SDL_BUTTON_MIDDLE:
        return 3;
    case SDL_BUTTON_X1:
        return 8;
    case SDL_BUTTON_X2:
        return 9;
    default:
        return 0;
    }
}

static bool mc_controller_send_godot_button(struct mc_controller* controller,
                                            uint8_t button,
                                            bool pressed,
                                            bool double_click,
                                            float x,
                                            float y) {
    uint8_t message[MC_GSI_HEADER_SIZE + 13];
    mc_gsi_header(message, 13);
    message[6] = MC_GSI_EVENT_MOUSE_BUTTON;
    message[7] = button;
    message[8] = pressed ? 1 : 0;
    message[9] = double_click ? 1 : 0;
    message[10] = 0;
    mc_write_f32_le(message + 11, x);
    mc_write_f32_le(message + 15, y);
    return mc_send(controller, message, sizeof(message));
}

void mc_controller_init(struct mc_controller* controller, struct mc_transport* transport) {
    controller->transport = transport;
}

bool mc_controller_send_key(struct mc_controller* controller, const SDL_KeyboardEvent* event) {
    if (event->repeat) {
        return true;
    }

    uint32_t keycode = mc_godot_key_from_sdl(event->key);
    if (!keycode) {
        return true;
    }

    uint8_t message[MC_GSI_HEADER_SIZE + 10];
    mc_gsi_header(message, 10);
    message[6] = MC_GSI_EVENT_KEY;
    message[7] = event->type == SDL_EVENT_KEY_DOWN ? 1 : 0;
    mc_write_u32_le(message + 8, keycode);
    mc_write_u32_le(message + 12, mc_unicode_from_sdl(event->key));
    return mc_send(controller, message, sizeof(message));
}

bool mc_controller_send_mouse_motion(
    struct mc_controller* controller, float x, float y, float dx, float dy) {
    if (!isfinite(x) || !isfinite(y) || !isfinite(dx) || !isfinite(dy)) {
        return true;
    }

    uint8_t message[MC_GSI_HEADER_SIZE + 17];
    mc_gsi_header(message, 17);
    message[6] = MC_GSI_EVENT_MOUSE_MOTION;
    mc_write_f32_le(message + 7, x);
    mc_write_f32_le(message + 11, y);
    mc_write_f32_le(message + 15, dx);
    mc_write_f32_le(message + 19, dy);
    return mc_send(controller, message, sizeof(message));
}

bool mc_controller_send_mouse_button(struct mc_controller* controller,
                                     uint8_t sdl_button,
                                     bool pressed,
                                     uint8_t clicks,
                                     float x,
                                     float y) {
    uint8_t button = mc_mouse_button_from_sdl(sdl_button);
    if (!button) {
        return true;
    }
    return mc_controller_send_godot_button(controller, button, pressed, clicks >= 2, x, y);
}

bool mc_controller_send_mouse_wheel(
    struct mc_controller* controller, float x, float y, float wheel_x, float wheel_y) {
    uint8_t vertical = wheel_y > 0.0f ? 4 : wheel_y < 0.0f ? 5 : 0;
    uint8_t horizontal = wheel_x < 0.0f ? 6 : wheel_x > 0.0f ? 7 : 0;
    uint8_t buttons[2] = {vertical, horizontal};

    for (size_t index = 0; index < 2; ++index) {
        uint8_t button = buttons[index];
        if (!button) {
            continue;
        }
        if (!mc_controller_send_godot_button(controller, button, true, false, x, y) ||
            !mc_controller_send_godot_button(controller, button, false, false, x, y)) {
            return false;
        }
    }
    return true;
}
