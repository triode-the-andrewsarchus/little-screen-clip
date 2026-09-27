#define _POSIX_C_SOURCE 200809L

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <linux/input-event-codes.h>
#include <wayland-client.h>

#include "virtual-pointer-client-protocol.h"

struct globals {
    struct wl_seat *seat;
    struct zwlr_virtual_pointer_manager_v1 *manager;
    uint32_t seat_name;
    uint32_t manager_name;
};

static void registry_global(void *data, struct wl_registry *registry,
                            uint32_t name, const char *interface,
                            uint32_t version) {
    struct globals *globals = data;
    if (strcmp(interface, wl_seat_interface.name) == 0 && globals->seat == NULL) {
        globals->seat = wl_registry_bind(registry, name, &wl_seat_interface,
                                         version < 1 ? version : 1);
        globals->seat_name = name;
    } else if (strcmp(interface, zwlr_virtual_pointer_manager_v1_interface.name) == 0 &&
               globals->manager == NULL) {
        globals->manager = wl_registry_bind(
            registry, name, &zwlr_virtual_pointer_manager_v1_interface,
            version < 1 ? version : 1);
        globals->manager_name = name;
    }
}

static void registry_global_remove(void *data, struct wl_registry *registry,
                                   uint32_t name) {
    (void)data;
    (void)registry;
    (void)name;
}

static const struct wl_registry_listener registry_listener = {
    .global = registry_global,
    .global_remove = registry_global_remove,
};

static void usage(const char *program) {
    fprintf(stderr,
            "usage: %s probe | click <width> <height> <x> <y> | "
            "scroll <width> <height> <x> <y> <logical-units>\n"
            "Connects only to WAYLAND_DISPLAY in XDG_RUNTIME_DIR.\n",
            program);
}

static int parse_positive(const char *text, uint32_t *value) {
    char *end = NULL;
    unsigned long parsed = strtoul(text, &end, 10);
    if (text[0] == '\0' || end == text || *end != '\0' || parsed == 0 ||
        parsed > UINT32_MAX) {
        return 0;
    }
    *value = (uint32_t)parsed;
    return 1;
}

static int parse_coordinate(const char *text, uint32_t extent, uint32_t *value) {
    char *end = NULL;
    unsigned long parsed = strtoul(text, &end, 10);
    if (text[0] == '\0' || end == text || *end != '\0' || parsed >= extent) {
        return 0;
    }
    *value = (uint32_t)parsed;
    return 1;
}

static int parse_scroll(const char *text, int *value) {
    char *end = NULL;
    long parsed = strtol(text, &end, 10);
    if (text[0] == '\0' || end == text || *end != '\0' || parsed == 0 ||
        parsed < -1000 || parsed > 1000) {
        return 0;
    }
    *value = (int)parsed;
    return 1;
}

int main(int argc, char **argv) {
    uint32_t width = 0, height = 0, x = 0, y = 0;
    int probe = argc == 2 && strcmp(argv[1], "probe") == 0;
    int click = argc == 6 && strcmp(argv[1], "click") == 0;
    int scroll = argc == 7 && strcmp(argv[1], "scroll") == 0;
    int scroll_amount = 0;
    if (!probe && !click && !scroll) {
        usage(argv[0]);
        return 2;
    }
    if ((click || scroll) &&
        (!parse_positive(argv[2], &width) || !parse_positive(argv[3], &height) ||
         !parse_coordinate(argv[4], width, &x) ||
         !parse_coordinate(argv[5], height, &y) ||
         (scroll && !parse_scroll(argv[6], &scroll_amount)))) {
        usage(argv[0]);
        return 2;
    }

    struct wl_display *display = wl_display_connect(NULL);
    if (display == NULL) {
        fprintf(stderr, "cannot connect to the runner's Wayland socket\n");
        return 3;
    }
    struct wl_registry *registry = wl_display_get_registry(display);
    struct globals globals = {0};
    wl_registry_add_listener(registry, &registry_listener, &globals);
    if (wl_display_roundtrip(display) < 0) {
        fprintf(stderr, "Wayland registry roundtrip failed\n");
        wl_display_disconnect(display);
        return 3;
    }

    if (globals.manager == NULL || globals.seat == NULL) {
        fprintf(stderr,
                "private compositor lacks virtual-pointer manager or wl_seat "
                "(manager=%d, seat=%d)\n",
                globals.manager != NULL, globals.seat != NULL);
        if (globals.manager != NULL) {
            zwlr_virtual_pointer_manager_v1_destroy(globals.manager);
        }
        if (globals.seat != NULL) {
            wl_seat_destroy(globals.seat);
        }
        wl_registry_destroy(registry);
        wl_display_disconnect(display);
        return 4;
    }

    if (probe) {
        printf("private virtual-pointer protocol available (manager global %u, seat global %u)\n",
               globals.manager_name, globals.seat_name);
    } else {
        struct zwlr_virtual_pointer_v1 *pointer =
            zwlr_virtual_pointer_manager_v1_create_virtual_pointer(
                globals.manager, globals.seat);
        if (pointer == NULL || wl_display_roundtrip(display) < 0) {
            fprintf(stderr, "failed to create private virtual pointer\n");
            if (pointer != NULL) {
                zwlr_virtual_pointer_v1_destroy(pointer);
            }
            zwlr_virtual_pointer_manager_v1_destroy(globals.manager);
            wl_seat_destroy(globals.seat);
            wl_registry_destroy(registry);
            wl_display_disconnect(display);
            return 5;
        }

        zwlr_virtual_pointer_v1_motion_absolute(pointer, 1, x, y, width, height);
        if (scroll) {
            zwlr_virtual_pointer_v1_axis_source(pointer, WL_POINTER_AXIS_SOURCE_WHEEL);
            zwlr_virtual_pointer_v1_axis(
                pointer, 2, WL_POINTER_AXIS_VERTICAL_SCROLL,
                wl_fixed_from_int(scroll_amount));
            zwlr_virtual_pointer_v1_frame(pointer);
            if (wl_display_roundtrip(display) < 0) {
                fprintf(stderr, "private pointer scroll failed\n");
                zwlr_virtual_pointer_v1_destroy(pointer);
                zwlr_virtual_pointer_manager_v1_destroy(globals.manager);
                wl_seat_destroy(globals.seat);
                wl_registry_destroy(registry);
                wl_display_disconnect(display);
                return 5;
            }
            printf("scrolled private Niri at %u,%u by %d logical units\n", x, y,
                   scroll_amount);
        } else {
            zwlr_virtual_pointer_v1_frame(pointer);
            if (wl_display_roundtrip(display) < 0) {
                fprintf(stderr, "private pointer motion failed\n");
                zwlr_virtual_pointer_v1_destroy(pointer);
                zwlr_virtual_pointer_manager_v1_destroy(globals.manager);
                wl_seat_destroy(globals.seat);
                wl_registry_destroy(registry);
                wl_display_disconnect(display);
                return 5;
            }
            zwlr_virtual_pointer_v1_button(pointer, 2, BTN_LEFT, WL_POINTER_BUTTON_STATE_PRESSED);
            zwlr_virtual_pointer_v1_frame(pointer);
            if (wl_display_roundtrip(display) < 0) {
                fprintf(stderr, "private pointer press failed\n");
                zwlr_virtual_pointer_v1_destroy(pointer);
                zwlr_virtual_pointer_manager_v1_destroy(globals.manager);
                wl_seat_destroy(globals.seat);
                wl_registry_destroy(registry);
                wl_display_disconnect(display);
                return 5;
            }
            zwlr_virtual_pointer_v1_button(pointer, 3, BTN_LEFT, WL_POINTER_BUTTON_STATE_RELEASED);
            zwlr_virtual_pointer_v1_frame(pointer);
            if (wl_display_roundtrip(display) < 0) {
                fprintf(stderr, "private pointer release failed\n");
                zwlr_virtual_pointer_v1_destroy(pointer);
                zwlr_virtual_pointer_manager_v1_destroy(globals.manager);
                wl_seat_destroy(globals.seat);
                wl_registry_destroy(registry);
                wl_display_disconnect(display);
                return 5;
            }
            printf("clicked private Niri logical coordinate %u,%u of %u×%u\n",
                   x, y, width, height);
        }
        zwlr_virtual_pointer_v1_destroy(pointer);
    }

    zwlr_virtual_pointer_manager_v1_destroy(globals.manager);
    wl_seat_destroy(globals.seat);
    wl_registry_destroy(registry);
    wl_display_disconnect(display);
    return 0;
}
