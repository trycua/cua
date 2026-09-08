/* Independent foreground adversary. Build with the generated wlr virtual
 * pointer client protocol and libwayland-client. This deliberately exercises
 * the PRIMARY seat in a disposable test session; background actions must go
 * through Cua Driver, not this program. A bounded hold always releases. */
#define _POSIX_C_SOURCE 200809L
#include <wayland-client.h>
#include "wlr-virtual-pointer-unstable-v1-client-protocol.h"
#include <errno.h>
#include <signal.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

static struct zwlr_virtual_pointer_manager_v1 *manager;
static volatile sig_atomic_t stopped;
static void stop(int sig) { (void)sig; stopped = 1; }
static void global(void *data, struct wl_registry *registry, uint32_t name,
                   const char *interface, uint32_t version) {
    (void)data;
    if (!strcmp(interface, "zwlr_virtual_pointer_manager_v1"))
        manager = wl_registry_bind(registry, name,
            &zwlr_virtual_pointer_manager_v1_interface, version < 2 ? version : 2);
}
static void removed(void *data, struct wl_registry *registry, uint32_t name) {
    (void)data; (void)registry; (void)name;
}
static const struct wl_registry_listener listener = {global, removed};
static uint32_t milliseconds(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (uint32_t)((uint64_t)t.tv_sec * 1000 + t.tv_nsec / 1000000);
}
static unsigned integer(const char *text) {
    char *end;
    errno = 0;
    unsigned long value = strtoul(text, &end, 10);
    if (errno || !*text || *end || value > 65535) exit(2);
    return (unsigned)value;
}
int main(int argc, char **argv) {
    if (argc != 6 && argc != 7) { fprintf(stderr, "usage: primary_grab X Y WIDTH HEIGHT HOLD_MS [canary|park|controlled]\n"); return 2; }
    int canary = argc == 7 && !strcmp(argv[6], "canary");
    int park = argc == 7 && !strcmp(argv[6], "park");
    int controlled = argc == 7 && !strcmp(argv[6], "controlled");
    if (argc == 7 && !canary && !park && !controlled) return 2;
    unsigned x = integer(argv[1]), y = integer(argv[2]);
    unsigned width = integer(argv[3]), height = integer(argv[4]), hold = integer(argv[5]);
    if (!width || !height || x >= width || y >= height || hold < 100 || hold > 60000) return 2;
    if (canary && (x + 40 >= width || y + 30 >= height)) return 2;
    signal(SIGTERM, stop); signal(SIGINT, stop);
    struct wl_display *display = wl_display_connect(NULL);
    if (!display) return 1;
    struct wl_registry *registry = wl_display_get_registry(display);
    wl_registry_add_listener(registry, &listener, NULL);
    if (wl_display_roundtrip(display) < 0 || !manager) return 1;
    struct zwlr_virtual_pointer_v1 *pointer = zwlr_virtual_pointer_manager_v1_create_virtual_pointer(manager, NULL);
    zwlr_virtual_pointer_v1_motion_absolute(pointer, milliseconds(), x, y, width, height);
    zwlr_virtual_pointer_v1_frame(pointer);
    if (wl_display_roundtrip(display) < 0) return 1;
    if (canary || park) {
        // Two warps, one flush/roundtrip: endpoint polling alone sees no change.
        if (canary) {
            zwlr_virtual_pointer_v1_motion_absolute(pointer, milliseconds(), x + 40, y + 30, width, height);
            zwlr_virtual_pointer_v1_motion_absolute(pointer, milliseconds(), x, y, width, height);
        }
        zwlr_virtual_pointer_v1_frame(pointer);
        int result = wl_display_roundtrip(display);
        zwlr_virtual_pointer_v1_destroy(pointer);
        zwlr_virtual_pointer_manager_v1_destroy(manager);
        wl_registry_destroy(registry);
        wl_display_flush(display);
        wl_display_disconnect(display);
        puts("CANARY_RETURNED");
        return result < 0;
    }
    zwlr_virtual_pointer_v1_button(pointer, milliseconds(), 272, WL_POINTER_BUTTON_STATE_PRESSED);
    zwlr_virtual_pointer_v1_frame(pointer);
    if (wl_display_roundtrip(display) < 0) return 1;
    puts("HELD"); fflush(stdout);
    uint32_t start = milliseconds();
    int bad_command = 0;
    while (!stopped && (uint32_t)(milliseconds() - start) < hold) {
        if (controlled) {
            struct pollfd input = {STDIN_FILENO, POLLIN, 0};
            int ready = poll(&input, 1, 10);
            if (ready < 0 && errno == EINTR) continue;
            if (ready < 0) { bad_command = 1; break; }
            if (!ready) continue;
            char line[128], extra;
            unsigned mx, my;
            if (!fgets(line, sizeof(line), stdin)) break;
            if (!strchr(line, '\n') || sscanf(line, "MOVE %u %u %c", &mx, &my, &extra) != 2 || mx >= width || my >= height) {
                bad_command = 1; break;
            }
            zwlr_virtual_pointer_v1_motion_absolute(pointer, milliseconds(), mx, my, width, height);
            zwlr_virtual_pointer_v1_frame(pointer);
            if (wl_display_roundtrip(display) < 0) { bad_command = 1; break; }
            printf("MOVED %u %u\n", mx, my); fflush(stdout);
            continue;
        }
        struct timespec pause = {0, 10000000};
        nanosleep(&pause, NULL);
    }
    zwlr_virtual_pointer_v1_button(pointer, milliseconds(), 272, WL_POINTER_BUTTON_STATE_RELEASED);
    zwlr_virtual_pointer_v1_frame(pointer);
    int result = wl_display_roundtrip(display);
    zwlr_virtual_pointer_v1_destroy(pointer);
    zwlr_virtual_pointer_manager_v1_destroy(manager);
    wl_registry_destroy(registry);
    wl_display_flush(display);
    wl_display_disconnect(display);
    puts("RELEASED");
    return result < 0 || bad_command;
}
