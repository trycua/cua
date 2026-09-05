/* Disposable-guest adversary, not an authentication/production lock screen.
 * Generate ext-session-lock-v1-client-protocol.h and protocol C with
 * wayland-scanner from wayland-protocols/staging/ext-session-lock/
 * ext-session-lock-v1.xml, then link this file and generated C with
 * pkg-config --cflags --libs wayland-client. Output binary: session_lock_fixture.
 *
 * Supply an opaque lock surface on each advertised output. Waiting for the
 * compositor's missing-surface fallback would measure its grace period rather
 * than a normally configured lock. Recovery sends unlock_and_destroy and waits
 * for a wl_display.sync acknowledgment. Never kill this client to unlock.
 */
#define _GNU_SOURCE
#include <wayland-client.h>
#include "ext-session-lock-v1-client-protocol.h"
#include <errno.h>
#include <poll.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>

static struct ext_session_lock_manager_v1 *manager;
static struct ext_session_lock_v1 *lock;
static struct wl_compositor *compositor;
static struct wl_shm *shm;
static struct output {
    struct wl_output *output;
    struct wl_surface *surface;
    struct ext_session_lock_surface_v1 *lock_surface;
} outputs[16];
static unsigned output_count;
static int locked, finished, synced;
static volatile sig_atomic_t release_requested;

static uint64_t now_ns(void) {
    struct timespec value;
    clock_gettime(CLOCK_MONOTONIC, &value);
    return (uint64_t)value.tv_sec * 1000000000ULL + value.tv_nsec;
}
static void event(const char *name) {
    printf("{\"event\":\"%s\",\"observed_ns\":%llu}\n", name,
           (unsigned long long)now_ns());
    fflush(stdout);
}
static void release(int sig) { (void)sig; release_requested = 1; }
static void buffer_released(void *data, struct wl_buffer *buffer) {
    (void)data; wl_buffer_destroy(buffer);
}
static const struct wl_buffer_listener buffer_listener = {buffer_released};
static void configure(void *data, struct ext_session_lock_surface_v1 *surface,
                      uint32_t serial, uint32_t width, uint32_t height) {
    struct output *output = data;
    if (!width || !height || width > 8192 || height > 8192) exit(1);
    size_t bytes = (size_t)width * height * 4;
    int fd = memfd_create("test-lock-black", MFD_CLOEXEC);
    if (fd < 0 || ftruncate(fd, (off_t)bytes)) exit(1);
    /* A fresh memfd is zero-filled. XRGB ignores alpha, so it is opaque black. */
    struct wl_shm_pool *pool = wl_shm_create_pool(shm, fd, (int32_t)bytes);
    struct wl_buffer *buffer = wl_shm_pool_create_buffer(pool, 0, (int32_t)width,
        (int32_t)height, (int32_t)width * 4, WL_SHM_FORMAT_XRGB8888);
    wl_shm_pool_destroy(pool); close(fd);
    wl_buffer_add_listener(buffer, &buffer_listener, NULL);
    ext_session_lock_surface_v1_ack_configure(surface, serial);
    wl_surface_attach(output->surface, buffer, 0, 0);
    wl_surface_damage(output->surface, 0, 0, (int32_t)width, (int32_t)height);
    wl_surface_commit(output->surface);
}
static const struct ext_session_lock_surface_v1_listener surface_listener = {configure};
static void global(void *data, struct wl_registry *registry, uint32_t name,
                   const char *interface, uint32_t version) {
    (void)data; (void)version;
    if (!strcmp(interface, "ext_session_lock_manager_v1"))
        manager = wl_registry_bind(registry, name, &ext_session_lock_manager_v1_interface, 1);
    else if (!strcmp(interface, "wl_compositor"))
        compositor = wl_registry_bind(registry, name, &wl_compositor_interface, 1);
    else if (!strcmp(interface, "wl_shm"))
        shm = wl_registry_bind(registry, name, &wl_shm_interface, 1);
    else if (!strcmp(interface, "wl_output")) {
        if (output_count == 16) exit(1);
        outputs[output_count++].output = wl_registry_bind(registry, name, &wl_output_interface, 1);
    }
}
static void removed(void *data, struct wl_registry *registry, uint32_t name) {
    (void)data; (void)registry; (void)name;
}
static const struct wl_registry_listener registry_listener = {global, removed};
static void on_locked(void *data, struct ext_session_lock_v1 *object) {
    (void)data; (void)object; locked = 1; event("locked");
}
static void on_finished(void *data, struct ext_session_lock_v1 *object) {
    (void)data; (void)object; finished = 1; event("finished");
}
static const struct ext_session_lock_v1_listener lock_listener = {on_locked, on_finished};
static void on_sync(void *data, struct wl_callback *callback, uint32_t serial) {
    (void)data; (void)serial; synced = 1; wl_callback_destroy(callback);
}
static const struct wl_callback_listener sync_listener = {on_sync};

/* All display waits are bounded. A hung compositor cannot wedge this helper
 * indefinitely; a missing unlocked acknowledgment is failure, never success. */
static int pump(struct wl_display *display, int watch_stdin) {
    if (wl_display_dispatch_pending(display) < 0) return -1;
    if (wl_display_flush(display) < 0 && errno != EAGAIN) return -1;
    struct pollfd fds[2] = {{wl_display_get_fd(display), POLLIN, 0},
                           {watch_stdin ? STDIN_FILENO : -1, POLLIN, 0}};
    int ready = poll(fds, 2, 20);
    if (ready < 0) return errno == EINTR ? 0 : -1;
    if (fds[0].revents & (POLLERR | POLLHUP | POLLNVAL)) return -1;
    if ((fds[0].revents & POLLIN) && wl_display_dispatch(display) < 0) return -1;
    if (fds[1].revents & (POLLIN | POLLHUP | POLLERR)) {
        char byte;
        /* Once LOCK has been read, any additional byte or EOF requests unlock. */
        (void)read(STDIN_FILENO, &byte, 1);
        release_requested = 1;
    }
    return 0;
}
static long number(const char *text, long min, long max) {
    char *end; errno = 0;
    long result = strtol(text, &end, 10);
    if (errno || !*text || *end || result < min || result > max) exit(2);
    return result;
}

int main(int argc, char **argv) {
    if (argc != 3) return 2;
    long duration = number(argv[1], 2000, 20000);
    pid_t expected = (pid_t)number(argv[2], 2, 2147483647);
    struct sigaction action = {.sa_handler = release};
    sigemptyset(&action.sa_mask);
    sigaction(SIGUSR1, &action, NULL);
    sigaction(SIGTERM, &action, NULL);
    sigaction(SIGINT, &action, NULL);
    signal(SIGPIPE, SIG_IGN);
    struct wl_display *display = wl_display_connect(NULL);
    if (!display) return 1;
    struct ucred peer;
    socklen_t size = sizeof(peer);
    if (getsockopt(wl_display_get_fd(display), SOL_SOCKET, SO_PEERCRED, &peer, &size) ||
        peer.pid != expected || peer.uid != getuid()) return 1;
    struct wl_registry *registry = wl_display_get_registry(display);
    wl_registry_add_listener(registry, &registry_listener, NULL);
    struct wl_callback *callback = wl_display_sync(display);
    wl_callback_add_listener(callback, &sync_listener, NULL);
    uint64_t deadline = now_ns() + 2000000000ULL;
    while (!synced && now_ns() < deadline && !release_requested)
        if (pump(display, 0) < 0) return 1;
    if (!synced || !manager || !compositor || !shm || !output_count || release_requested) return 1;
    event("ready");
    /* Byte-wise bounded input prevents an incomplete command from blocking the
     * independent deadline. No arbitrary commands or real credentials exist. */
    char command[5]; unsigned used = 0;
    deadline = now_ns() + 3000000000ULL;
    while (used < sizeof(command) && now_ns() < deadline && !release_requested) {
        struct pollfd input = {STDIN_FILENO, POLLIN, 0};
        if (poll(&input, 1, 20) < 0 && errno != EINTR) return 1;
        if (input.revents & (POLLIN | POLLHUP)) {
            if (read(STDIN_FILENO, command + used, 1) != 1) return 1;
            ++used;
        }
    }
    if (used != sizeof(command) || memcmp(command, "LOCK\n", sizeof(command)) || release_requested) return 1;
    lock = ext_session_lock_manager_v1_lock(manager);
    ext_session_lock_v1_add_listener(lock, &lock_listener, NULL);
    for (unsigned i = 0; i < output_count; ++i) {
        outputs[i].surface = wl_compositor_create_surface(compositor);
        outputs[i].lock_surface = ext_session_lock_v1_get_lock_surface(lock,
            outputs[i].surface, outputs[i].output);
        ext_session_lock_surface_v1_add_listener(outputs[i].lock_surface,
            &surface_listener, &outputs[i]);
    }
    deadline = now_ns() + (uint64_t)duration * 1000000ULL;
    while (!finished && !locked && now_ns() < deadline)
        if (pump(display, 1) < 0) return 1;
    if (finished) { ext_session_lock_v1_destroy(lock); wl_display_flush(display); return 1; }
    if (!locked) return 1; /* No protocol unlock is legal before locked. */
    while (!release_requested && now_ns() < deadline)
        if (pump(display, 1) < 0) return 1;
    ext_session_lock_v1_unlock_and_destroy(lock);
    synced = 0;
    callback = wl_display_sync(display);
    wl_callback_add_listener(callback, &sync_listener, NULL);
    deadline = now_ns() + 2000000000ULL;
    while (!synced && now_ns() < deadline)
        if (pump(display, 0) < 0) return 1;
    if (!synced) return 1;
    event("unlocked");
    for (unsigned i = 0; i < output_count; ++i) {
        ext_session_lock_surface_v1_destroy(outputs[i].lock_surface);
        wl_surface_destroy(outputs[i].surface);
        wl_output_destroy(outputs[i].output);
    }
    ext_session_lock_manager_v1_destroy(manager);
    wl_shm_destroy(shm);
    wl_compositor_destroy(compositor);
    wl_registry_destroy(registry);
    wl_display_flush(display);
    wl_display_disconnect(display);
    return 0;
}
