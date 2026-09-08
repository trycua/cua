/* TEST ONLY: one explicit primary hover, never a primary button/key.
 * Build with the same generated virtual-pointer protocol as primary_grab.c.
 * Usage: primary_hover_fixture WIDTH HEIGHT LIFETIME_MS COMPOSITOR_PID
 * READY creates no input. One MOVE X Y command sends motion+frame and waits
 * for display.sync before reporting moved. The caller must independently
 * attest the actual focused client; a sync is not a focus acknowledgement.
 * EOF, signals and a bounded deadline destroy only this fixture's pointer.
 */
#define _GNU_SOURCE
#include <wayland-client.h>
#include "wlr-virtual-pointer-unstable-v1-client-protocol.h"
#include <errno.h>
#include <poll.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <time.h>
#include <unistd.h>

static struct zwlr_virtual_pointer_manager_v1 *manager;
static volatile sig_atomic_t stopped;
static int synced;
static uint64_t now_ns(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (uint64_t)t.tv_sec * 1000000000ULL + t.tv_nsec;
}
static void stop(int sig) { (void)sig; stopped = 1; }
static void event(const char *name, unsigned x, unsigned y) {
    printf("{\"event\":\"%s\",\"x\":%u,\"y\":%u,\"observed_ns\":%llu}\n",
           name, x, y, (unsigned long long)now_ns());
    fflush(stdout);
}
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
static void on_sync(void *data, struct wl_callback *callback, uint32_t serial) {
    (void)data; (void)serial; synced = 1; wl_callback_destroy(callback);
}
static const struct wl_callback_listener sync_listener = {on_sync};
static void start_sync(struct wl_display *display) {
    synced = 0;
    wl_callback_add_listener(wl_display_sync(display), &sync_listener, NULL);
}
/* No roundtrip or line-buffered stdin wait may outlive the deadline. */
static int pump(struct wl_display *display) {
    if (wl_display_dispatch_pending(display) < 0) return -1;
    if (wl_display_flush(display) < 0 && errno != EAGAIN) return -1;
    struct pollfd fd = {wl_display_get_fd(display), POLLIN, 0};
    int result = poll(&fd, 1, 10);
    if (result < 0) return errno == EINTR ? 0 : -1;
    if (fd.revents & (POLLERR | POLLHUP | POLLNVAL)) return -1;
    return (fd.revents & POLLIN) ? wl_display_dispatch(display) : 0;
}
static unsigned number(const char *text, unsigned min, unsigned max) {
    char *end; errno = 0;
    if (!*text || strspn(text, "0123456789") != strlen(text)) exit(2);
    unsigned long value = strtoul(text, &end, 10);
    if (errno || *end || value < min || value > max) exit(2);
    return (unsigned)value;
}
int main(int argc, char **argv) {
    if (argc != 5) return 2;
    unsigned width = number(argv[1], 1, 65535), height = number(argv[2], 1, 65535);
    unsigned lifetime = number(argv[3], 2000, 20000);
    pid_t expected = (pid_t)number(argv[4], 2, 2147483647);
    signal(SIGTERM, stop); signal(SIGINT, stop); signal(SIGPIPE, SIG_IGN);
    struct wl_display *display = wl_display_connect(NULL);
    if (!display) return 1;
    struct ucred peer;
    socklen_t size = sizeof(peer);
    if (getsockopt(wl_display_get_fd(display), SOL_SOCKET, SO_PEERCRED, &peer, &size) ||
        size != sizeof(peer) || peer.pid != expected || peer.uid != getuid()) return 1;
    struct wl_registry *registry = wl_display_get_registry(display);
    wl_registry_add_listener(registry, &listener, NULL);
    start_sync(display);
    uint64_t deadline = now_ns() + 2000000000ULL;
    while (!synced && !stopped && now_ns() < deadline)
        if (pump(display) < 0) return 1;
    if (!synced || !manager || stopped) return 1;
    struct zwlr_virtual_pointer_v1 *pointer = zwlr_virtual_pointer_manager_v1_create_virtual_pointer(manager, NULL);
    start_sync(display);
    deadline = now_ns() + 2000000000ULL;
    while (!synced && !stopped && now_ns() < deadline)
        if (pump(display) < 0) return 1;
    if (!synced || stopped) return 1;
    event("ready", 0, 0);
    char command[48] = {0}; unsigned used = 0;
    deadline = now_ns() + 3000000000ULL;
    while (used < sizeof(command) - 1 && !stopped && now_ns() < deadline) {
        if (pump(display) < 0) return 1;
        struct pollfd input = {STDIN_FILENO, POLLIN, 0};
        if (poll(&input, 1, 10) < 0 && errno != EINTR) return 1;
        if (input.revents & (POLLIN | POLLHUP | POLLERR)) {
            if (read(STDIN_FILENO, command + used, 1) != 1) return 1;
            if (command[used++] == '\n') break;
        }
    }
    unsigned x, y; char extra;
    if (!used || command[used - 1] != '\n' || stopped || now_ns() >= deadline ||
        sscanf(command, "MOVE %u %u %c", &x, &y, &extra) != 2 || x >= width || y >= height) return 1;
    /* This is the only input request in the entire fixture. */
    zwlr_virtual_pointer_v1_motion_absolute(pointer, (uint32_t)(now_ns() / 1000000ULL), x, y, width, height);
    zwlr_virtual_pointer_v1_frame(pointer);
    start_sync(display);
    deadline = now_ns() + 2000000000ULL;
    while (!synced && !stopped && now_ns() < deadline)
        if (pump(display) < 0) return 1;
    if (!synced || stopped) return 1;
    event("moved", x, y);
    int failed = 0, closed = 0;
    deadline = now_ns() + (uint64_t)lifetime * 1000000ULL;
    while (!stopped && now_ns() < deadline) {
        if (pump(display) < 0) { failed = 1; break; }
        struct pollfd input = {STDIN_FILENO, POLLIN, 0};
        if (poll(&input, 1, 10) < 0 && errno != EINTR) { failed = 1; break; }
        if (input.revents & (POLLIN | POLLHUP | POLLERR)) {
            char byte;
            /* A second command is an error, never another movement. */
            closed = read(STDIN_FILENO, &byte, 1) == 0;
            failed = !closed;
            break;
        }
    }
    zwlr_virtual_pointer_v1_destroy(pointer);
    zwlr_virtual_pointer_manager_v1_destroy(manager);
    wl_registry_destroy(registry);
    wl_display_flush(display);
    wl_display_disconnect(display);
    event("finished", x, y);
    return failed || !closed; /* Deadline/signal cleanup cannot certify success. */
}
