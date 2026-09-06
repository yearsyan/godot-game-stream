// SPDX-License-Identifier: LGPL-2.1-or-later
// Copyright (C) 2026 yearsyan and contributors
#include "control.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static uint8_t sent[128];
static size_t sent_size;

bool mc_transport_write_all(struct mc_transport* transport, const uint8_t* data, size_t size) {
    (void)transport;
    if (size > sizeof(sent)) {
        return false;
    }
    memcpy(sent, data, size);
    sent_size = size;
    return true;
}

static void check_fixture(const char* name, bool ok) {
    char path[2048];
    snprintf(path, sizeof(path), "%s/%s", MC_FIXTURES_PATH, name);
    FILE* file = fopen(path, "rb");
    if (!file || !ok) {
        fprintf(stderr, "Cannot read fixture or send event: %s\n", path);
        exit(1);
    }
    uint8_t expected[128];
    size_t size = fread(expected, 1, sizeof(expected), file);
    bool read_error = ferror(file) != 0;
    fclose(file);
    if (read_error || size != sent_size || memcmp(expected, sent, size) != 0) {
        fprintf(stderr, "GSI1 fixture mismatch: %s\n", name);
        exit(1);
    }
}

int main(void) {
    struct mc_transport transport = {0};
    struct mc_controller controller;
    mc_controller_init(&controller, &transport);
    SDL_KeyboardEvent key = {0};
    key.type = SDL_EVENT_KEY_DOWN;
    key.key = SDLK_A;
    check_fixture("key-a.bin", mc_controller_send_key(&controller, &key));
    check_fixture("mouse-left.bin", mc_controller_send_mouse_button(
        &controller, SDL_BUTTON_LEFT, true, 1, 100.5f, 200.25f));
    check_fixture("mouse-motion.bin", mc_controller_send_mouse_motion(
        &controller, 100.5f, 200.25f, -2.0f, 3.5f));
    puts("All GSI1 shared fixtures match.");
    return 0;
}
