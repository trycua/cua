// A Wayland client that binds only the first wl_seat it is offered, as
// Chromium's Ozone Wayland backend does (and so Electron). It maps one window
// and appends what its seat delivers to a log file, one line per event:
//
//   seat <name>              the bound seat's name
//   button <code> <0|1>      pointer button released / pressed
//   key <code> <0|1>         key released / pressed
//
// usage: first_seat_client <log-file> <app-id>
//
// requalify/headless.py uses it to check that a client the driver tagged
// (CUA_AGENT_SEAT) is handed an agent seat as its only wl_seat, and that an
// untagged one still gets the user's seat.
#define _GNU_SOURCE
#include <errno.h>
#include <stdarg.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#include <wayland-client.h>
#include "xdg-shell-client-protocol.h"

enum { WIDTH = 300, HEIGHT = 200 };

static FILE *log_file;
static struct wl_compositor *compositor;
static struct wl_shm *shm;
static struct xdg_wm_base *wm_base;
static struct wl_seat *seat;
static struct wl_surface *surface;
static struct wl_pointer *pointer;
static struct wl_keyboard *keyboard;
static const char *app_id;

static void say(const char *format, ...) __attribute__((format(printf, 1, 2)));
static void say(const char *format, ...) {
    va_list args;
    va_start(args, format);
    vfprintf(log_file, format, args);
    va_end(args);
    fputc('\n', log_file);
    fflush(log_file);
}

static void ping(void *data, struct xdg_wm_base *base, uint32_t serial) {
    (void)data;
    xdg_wm_base_pong(base, serial);
}
static const struct xdg_wm_base_listener wm_listener = {ping};

static void pointer_enter(void *d, struct wl_pointer *p, uint32_t s, struct wl_surface *f, wl_fixed_t x, wl_fixed_t y) { (void)d; (void)p; (void)s; (void)f; (void)x; (void)y; }
static void pointer_leave(void *d, struct wl_pointer *p, uint32_t s, struct wl_surface *f) { (void)d; (void)p; (void)s; (void)f; }
static void pointer_motion(void *d, struct wl_pointer *p, uint32_t t, wl_fixed_t x, wl_fixed_t y) { (void)d; (void)p; (void)t; (void)x; (void)y; }
static void pointer_button(void *d, struct wl_pointer *p, uint32_t s, uint32_t t, uint32_t b, uint32_t state) {
    (void)d; (void)p; (void)s; (void)t;
    say("button %u %u", b, state == WL_POINTER_BUTTON_STATE_PRESSED);
}
static void pointer_axis(void *d, struct wl_pointer *p, uint32_t t, uint32_t a, wl_fixed_t v) { (void)d; (void)p; (void)t; (void)a; (void)v; }
static void pointer_frame(void *d, struct wl_pointer *p) { (void)d; (void)p; }
static void pointer_axis_source(void *d, struct wl_pointer *p, uint32_t source) { (void)d; (void)p; (void)source; }
static void pointer_axis_stop(void *d, struct wl_pointer *p, uint32_t t, uint32_t a) { (void)d; (void)p; (void)t; (void)a; }
static void pointer_axis_discrete(void *d, struct wl_pointer *p, uint32_t a, int32_t v) { (void)d; (void)p; (void)a; (void)v; }
static const struct wl_pointer_listener pointer_listener = {
    .enter = pointer_enter, .leave = pointer_leave, .motion = pointer_motion, .button = pointer_button, .axis = pointer_axis,
    .frame = pointer_frame, .axis_source = pointer_axis_source, .axis_stop = pointer_axis_stop,
    .axis_discrete = pointer_axis_discrete,
};

static void keyboard_keymap(void *d, struct wl_keyboard *k, uint32_t format, int fd, uint32_t size) { (void)d; (void)k; (void)format; (void)size; close(fd); }
static void keyboard_enter(void *d, struct wl_keyboard *k, uint32_t s, struct wl_surface *f, struct wl_array *keys) { (void)d; (void)k; (void)s; (void)f; (void)keys; }
static void keyboard_leave(void *d, struct wl_keyboard *k, uint32_t s, struct wl_surface *f) { (void)d; (void)k; (void)s; (void)f; }
static void keyboard_key(void *d, struct wl_keyboard *k, uint32_t s, uint32_t t, uint32_t key, uint32_t state) {
    (void)d; (void)k; (void)s; (void)t;
    say("key %u %u", key, state == WL_KEYBOARD_KEY_STATE_PRESSED);
}
static void keyboard_modifiers(void *d, struct wl_keyboard *k, uint32_t s, uint32_t a, uint32_t b, uint32_t c, uint32_t g) { (void)d; (void)k; (void)s; (void)a; (void)b; (void)c; (void)g; }
static void keyboard_repeat(void *d, struct wl_keyboard *k, int32_t rate, int32_t delay) { (void)d; (void)k; (void)rate; (void)delay; }
static const struct wl_keyboard_listener keyboard_listener = {
    .keymap = keyboard_keymap, .enter = keyboard_enter, .leave = keyboard_leave, .key = keyboard_key,
    .modifiers = keyboard_modifiers, .repeat_info = keyboard_repeat,
};

static void seat_capabilities(void *d, struct wl_seat *s, uint32_t caps) {
    (void)d;
    if ((caps & WL_SEAT_CAPABILITY_POINTER) && !pointer) {
        pointer = wl_seat_get_pointer(s);
        wl_pointer_add_listener(pointer, &pointer_listener, NULL);
    }
    if ((caps & WL_SEAT_CAPABILITY_KEYBOARD) && !keyboard) {
        keyboard = wl_seat_get_keyboard(s);
        wl_keyboard_add_listener(keyboard, &keyboard_listener, NULL);
    }
}
static void seat_name(void *d, struct wl_seat *s, const char *name) { (void)d; (void)s; say("seat %s", name); }
static const struct wl_seat_listener seat_listener = {seat_capabilities, seat_name};

static void global(void *d, struct wl_registry *registry, uint32_t name, const char *interface, uint32_t version) {
    (void)d;
    if (!strcmp(interface, wl_compositor_interface.name)) {
        compositor = wl_registry_bind(registry, name, &wl_compositor_interface, 4);
    } else if (!strcmp(interface, wl_shm_interface.name)) {
        shm = wl_registry_bind(registry, name, &wl_shm_interface, 1);
    } else if (!strcmp(interface, xdg_wm_base_interface.name)) {
        wm_base = wl_registry_bind(registry, name, &xdg_wm_base_interface, 1);
        xdg_wm_base_add_listener(wm_base, &wm_listener, NULL);
    } else if (!strcmp(interface, wl_seat_interface.name) && !seat) {
        // Only the first wl_seat, and only once: the Chromium behavior.
        seat = wl_registry_bind(registry, name, &wl_seat_interface, version < 7 ? version : 7);
        wl_seat_add_listener(seat, &seat_listener, NULL);
    }
}
static void global_remove(void *d, struct wl_registry *r, uint32_t name) { (void)d; (void)r; (void)name; }
static const struct wl_registry_listener registry_listener = {global, global_remove};

static void surface_configure(void *d, struct xdg_surface *xs, uint32_t serial) {
    (void)d;
    xdg_surface_ack_configure(xs, serial);
}
static const struct xdg_surface_listener xdg_surface_listener = {surface_configure};
static void toplevel_configure(void *d, struct xdg_toplevel *t, int32_t w, int32_t h, struct wl_array *states) { (void)d; (void)t; (void)w; (void)h; (void)states; }
static void toplevel_close(void *d, struct xdg_toplevel *t) { (void)d; (void)t; exit(0); }
static const struct xdg_toplevel_listener toplevel_listener = {toplevel_configure, toplevel_close};

int main(int argc, char **argv) {
    if (argc != 3) {
        fprintf(stderr, "usage: %s <log-file> <app-id>\n", argv[0]);
        return 2;
    }
    app_id = argv[2];
    log_file = fopen(argv[1], "a");
    if (!log_file) { perror("log"); return 1; }
    struct wl_display *display = wl_display_connect(NULL);
    if (!display) { fprintf(stderr, "no wayland display\n"); return 1; }
    struct wl_registry *registry = wl_display_get_registry(display);
    wl_registry_add_listener(registry, &registry_listener, NULL);
    wl_display_roundtrip(display);
    if (!compositor || !shm || !wm_base || !seat) { fprintf(stderr, "missing globals\n"); return 1; }
    wl_display_roundtrip(display);

    int fd = memfd_create("first-seat-client", MFD_CLOEXEC);
    if (fd < 0 || ftruncate(fd, WIDTH * HEIGHT * 4) != 0) { perror("buffer"); return 1; }
    uint32_t *pixels = mmap(NULL, WIDTH * HEIGHT * 4, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (pixels == MAP_FAILED) { perror("mmap"); return 1; }
    for (int i = 0; i < WIDTH * HEIGHT; i++) pixels[i] = 0xff336699;
    struct wl_shm_pool *pool = wl_shm_create_pool(shm, fd, WIDTH * HEIGHT * 4);
    struct wl_buffer *buffer = wl_shm_pool_create_buffer(pool, 0, WIDTH, HEIGHT, WIDTH * 4, WL_SHM_FORMAT_ARGB8888);

    surface = wl_compositor_create_surface(compositor);
    struct xdg_surface *xs = xdg_wm_base_get_xdg_surface(wm_base, surface);
    xdg_surface_add_listener(xs, &xdg_surface_listener, NULL);
    struct xdg_toplevel *toplevel = xdg_surface_get_toplevel(xs);
    xdg_toplevel_add_listener(toplevel, &toplevel_listener, NULL);
    xdg_toplevel_set_app_id(toplevel, app_id);
    xdg_toplevel_set_title(toplevel, app_id);
    wl_surface_commit(surface);
    wl_display_roundtrip(display);
    wl_surface_attach(surface, buffer, 0, 0);
    wl_surface_damage(surface, 0, 0, WIDTH, HEIGHT);
    wl_surface_commit(surface);

    while (wl_display_dispatch(display) != -1) {}
    return 0;
}
