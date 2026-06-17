#!/usr/bin/env python3
# Transforms wlroots' upstream `tinywl/tinywl.c` (0.19) into `cua-compositor.c`:
# a minimal headless wlroots compositor that cua-driver spawns as its nested
# session (CUA_WAYLAND_NEST_COMPOSITOR=cua-compositor) to perform what stock
# Wayland forbids a client from doing —
#
#   * focus-FREE per-surface KEYBOARD injection (type into an unfocused window),
#   * MULTI-cursor pointer injection (N independent cursors on one window),
#
# both routed to a target window by its xdg app_id, driven over a tiny line
# protocol on a unix control socket ($CUA_INJECT_SOCKET). It also exposes
# foreign-toplevel-management (so cua-driver's list_windows enumerates windows)
# and screencopy (so grim captures the output) — the same protocols labwc gives
# the non-nested cells — so the EIS cells keep working end to end.
#
# The injection primitives (cua_motion/cua_button/cua_kbd_*) deliver wl_pointer/
# wl_keyboard straight to a target client's resources, bypassing seat focus —
# routed by app_id (not positional device index) and transported over a plain
# socket instead of libei/EIS, since cua owns both ends and the portal/libei
# layer buys nothing here.
#
# Usage: cua_compositor_patch.py <tinywl.c in> <cua-compositor.c out>
import sys, io

inp = sys.argv[1] if len(sys.argv) > 1 else "tinywl.c"
out = sys.argv[2] if len(sys.argv) > 2 else "cua-compositor.c"
src = io.open(inp, encoding="utf-8").read()

# memfd_create + SOCK_CLOEXEC etc. need _GNU_SOURCE before any system header.
src = "#define _GNU_SOURCE\n" + src

INCLUDES = r"""#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <errno.h>
#include <time.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <linux/input-event-codes.h>
#include <wayland-server-protocol.h>
#include <xkbcommon/xkbcommon.h>
#include <wlr/types/wlr_foreign_toplevel_management_v1.h>
#include <wlr/types/wlr_screencopy_v1.h>
#include <wlr/types/wlr_xdg_output_v1.h>

#define CUA_MAXDEV 64
/* Per-cursor / per-keyboard enter bookkeeping (idx = logical device). */
struct cua_devstate { struct wlr_surface *entered; };
static struct cua_devstate cua_ptr[CUA_MAXDEV];
static struct cua_devstate cua_kbd_state[CUA_MAXDEV];
static int g_keymap_fd = -1;
static size_t g_keymap_size = 0;
static xkb_mod_mask_t g_shift_mask = 1;
struct cua_keyent { uint32_t keycode; int shift; int valid; };
static struct cua_keyent g_chartab[128];
static struct wlr_foreign_toplevel_manager_v1 *g_ftl_mgr = NULL;

"""

# A foreign-toplevel handle pointer on each toplevel (for list_windows).
STRUCT_FIELD = (
    "\tstruct wlr_xdg_toplevel *xdg_toplevel;\n"
    "\tstruct wlr_foreign_toplevel_handle_v1 *ftl;\n"
)

FUNCS = r"""
static uint32_t cua_now_ms(void) {
	struct timespec ts; clock_gettime(CLOCK_MONOTONIC, &ts);
	return (uint32_t)(ts.tv_sec * 1000 + ts.tv_nsec / 1000000);
}
static void cua_pframe(struct wl_resource *res) {
	if (wl_resource_get_version(res) >= WL_POINTER_FRAME_SINCE_VERSION)
		wl_pointer_send_frame(res);
}
/* Resolve a target window by its xdg app_id. */
static struct tinywl_toplevel *cua_find_appid(struct tinywl_server *server, const char *app_id) {
	struct tinywl_toplevel *t;
	wl_list_for_each(t, &server->toplevels, link) {
		const char *a = t->xdg_toplevel ? t->xdg_toplevel->app_id : NULL;
		if (a && strcmp(a, app_id) == 0) return t;
	}
	return NULL;
}
static void cua_ptr_leave(struct wlr_seat *seat, struct wlr_surface *surf) {
	if (!surf) return;
	struct wlr_seat_client *sc = wlr_seat_client_for_wl_client(seat, wl_resource_get_client(surf->resource));
	if (!sc) return;
	uint32_t serial = wlr_seat_client_next_serial(sc);
	struct wl_resource *res;
	wl_resource_for_each(res, &sc->pointers) { wl_pointer_send_leave(res, serial, surf->resource); cua_pframe(res); }
}
/* Inject pointer motion from logical cursor `idx` into window `t` at window-
 * local (x,y). enter/leave is tracked per idx, so several idx values can drive
 * independent cursors against the same or different surfaces. Focus-free: we
 * write straight to the client's wl_pointer resources, never touching the seat
 * focus or any real cursor. */
static void cua_motion(struct tinywl_server *server, struct tinywl_toplevel *t, int idx, double x, double y) {
	if (!t || idx < 0 || idx >= CUA_MAXDEV) return;
	struct wlr_surface *surface = t->xdg_toplevel->base->surface;
	struct wlr_box geo = t->xdg_toplevel->base->geometry;
	wl_fixed_t sx = wl_fixed_from_double(geo.x + x), sy = wl_fixed_from_double(geo.y + y);
	struct wlr_seat_client *sc = wlr_seat_client_for_wl_client(server->seat, wl_resource_get_client(surface->resource));
	if (!sc || wl_list_empty(&sc->pointers)) return;
	struct wl_resource *res;
	if (cua_ptr[idx].entered != surface) {
		if (cua_ptr[idx].entered) cua_ptr_leave(server->seat, cua_ptr[idx].entered);
		uint32_t es = wlr_seat_client_next_serial(sc);
		wl_resource_for_each(res, &sc->pointers) { wl_pointer_send_enter(res, es, surface->resource, sx, sy); cua_pframe(res); }
		cua_ptr[idx].entered = surface;
	}
	uint32_t tm = cua_now_ms();
	wl_resource_for_each(res, &sc->pointers) { wl_pointer_send_motion(res, tm, sx, sy); cua_pframe(res); }
}
static void cua_button(struct tinywl_server *server, struct tinywl_toplevel *t, int idx, uint32_t button, bool pressed) {
	if (!t || idx < 0 || idx >= CUA_MAXDEV) return;
	struct wlr_surface *surface = t->xdg_toplevel->base->surface;
	struct wlr_seat_client *sc = wlr_seat_client_for_wl_client(server->seat, wl_resource_get_client(surface->resource));
	if (!sc || wl_list_empty(&sc->pointers)) return;
	uint32_t tm = cua_now_ms(), bs = wlr_seat_client_next_serial(sc);
	struct wl_resource *res;
	wl_resource_for_each(res, &sc->pointers) {
		wl_pointer_send_button(res, bs, tm, button, pressed ? WL_POINTER_BUTTON_STATE_PRESSED : WL_POINTER_BUTTON_STATE_RELEASED);
		cua_pframe(res);
	}
}
static void cua_init_keymap(void) {
	struct xkb_context *ctx = xkb_context_new(XKB_CONTEXT_NO_FLAGS);
	struct xkb_rule_names names = {0};
	struct xkb_keymap *km = xkb_keymap_new_from_names(ctx, &names, XKB_KEYMAP_COMPILE_NO_FLAGS);
	char *s = xkb_keymap_get_as_string(km, XKB_KEYMAP_FORMAT_TEXT_V1);
	g_keymap_size = strlen(s) + 1;
	g_keymap_fd = memfd_create("cua-keymap", MFD_CLOEXEC);
	if (ftruncate(g_keymap_fd, g_keymap_size) == 0) {
		void *p = mmap(NULL, g_keymap_size, PROT_READ | PROT_WRITE, MAP_SHARED, g_keymap_fd, 0);
		memcpy(p, s, g_keymap_size); munmap(p, g_keymap_size);
	}
	free(s);
	/* Build a US-layout char -> (evdev keycode, shift-level) table from the
	 * keymap so the `t` command can type arbitrary ASCII. xkb keycodes are
	 * evdev+8; wl_keyboard.key wants the evdev code. */
	xkb_mod_index_t shift = xkb_keymap_mod_get_index(km, XKB_MOD_NAME_SHIFT);
	if (shift != XKB_MOD_INVALID) g_shift_mask = (xkb_mod_mask_t)1 << shift;
	for (xkb_keycode_t kc = 9; kc < 256; kc++) {
		for (int lvl = 0; lvl < 2; lvl++) {
			const xkb_keysym_t *syms;
			int n = xkb_keymap_key_get_syms_by_level(km, kc, 0, lvl, &syms);
			if (n != 1) continue;
			uint32_t cp = xkb_keysym_to_utf32(syms[0]);
			if (cp > 0 && cp < 128 && !g_chartab[cp].valid) {
				g_chartab[cp].keycode = (uint32_t)kc - 8;
				g_chartab[cp].shift = lvl;
				g_chartab[cp].valid = 1;
			}
		}
	}
	/* Control characters used by the protocol map to dedicated keys. */
	g_chartab['\n'] = (struct cua_keyent){ KEY_ENTER, 0, 1 };
	g_chartab['\t'] = (struct cua_keyent){ KEY_TAB, 0, 1 };
	xkb_keymap_unref(km); xkb_context_unref(ctx);
	wlr_log(WLR_INFO, "[cua] xkb keymap + chartab ready (%zu bytes)", g_keymap_size);
}
/* Ensure the target client's keyboard has had keymap + enter sent once (focus
 * free). idx 0 is the typing keyboard. */
static struct wlr_seat_client *cua_kbd_enter(struct tinywl_server *server, struct tinywl_toplevel *t) {
	struct wlr_surface *surface = t->xdg_toplevel->base->surface;
	struct wlr_seat_client *sc = wlr_seat_client_for_wl_client(server->seat, wl_resource_get_client(surface->resource));
	if (!sc || wl_list_empty(&sc->keyboards)) return NULL;
	if (cua_kbd_state[0].entered != surface) {
		struct wl_resource *res; struct wl_array keys; wl_array_init(&keys);
		wl_resource_for_each(res, &sc->keyboards) {
			wl_keyboard_send_keymap(res, WL_KEYBOARD_KEYMAP_FORMAT_XKB_V1, g_keymap_fd, g_keymap_size);
			wl_keyboard_send_enter(res, wlr_seat_client_next_serial(sc), surface->resource, &keys);
			wl_keyboard_send_modifiers(res, wlr_seat_client_next_serial(sc), 0, 0, 0, 0);
		}
		wl_array_release(&keys);
		cua_kbd_state[0].entered = surface;
	}
	return sc;
}
static void cua_kbd_mods(struct wlr_seat_client *sc, uint32_t depressed) {
	struct wl_resource *res;
	wl_resource_for_each(res, &sc->keyboards)
		wl_keyboard_send_modifiers(res, wlr_seat_client_next_serial(sc), depressed, 0, 0, 0);
}
static void cua_kbd_key(struct wlr_seat_client *sc, uint32_t keycode, bool pressed) {
	uint32_t tm = cua_now_ms();
	struct wl_resource *res;
	wl_resource_for_each(res, &sc->keyboards)
		wl_keyboard_send_key(res, wlr_seat_client_next_serial(sc), tm, keycode,
			pressed ? WL_KEYBOARD_KEY_STATE_PRESSED : WL_KEYBOARD_KEY_STATE_RELEASED);
}
static void cua_type_cp(struct tinywl_server *server, struct tinywl_toplevel *t, uint32_t cp) {
	if (cp >= 128 || !g_chartab[cp].valid) return;
	struct wlr_seat_client *sc = cua_kbd_enter(server, t);
	if (!sc) return;
	struct cua_keyent e = g_chartab[cp];
	if (e.shift) cua_kbd_mods(sc, g_shift_mask);
	cua_kbd_key(sc, e.keycode, true);
	cua_kbd_key(sc, e.keycode, false);
	if (e.shift) cua_kbd_mods(sc, 0);
}
/* Decode a hex-encoded ASCII string and type it focus-free into `t`. */
static void cua_type_hex(struct tinywl_server *server, struct tinywl_toplevel *t, const char *hex) {
	if (!t) return;
	for (const char *p = hex; p[0] && p[1]; p += 2) {
		int hi = (p[0] <= '9') ? p[0] - '0' : (p[0] | 0x20) - 'a' + 10;
		int lo = (p[1] <= '9') ? p[1] - '0' : (p[1] | 0x20) - 'a' + 10;
		cua_type_cp(server, t, (uint32_t)((hi << 4) | lo));
	}
}
static void cua_key_named(struct tinywl_server *server, struct tinywl_toplevel *t, const char *name) {
	if (!t) return;
	uint32_t kc = 0;
	if (!strcasecmp(name, "enter") || !strcasecmp(name, "return")) kc = KEY_ENTER;
	else if (!strcasecmp(name, "tab")) kc = KEY_TAB;
	else if (!strcasecmp(name, "escape") || !strcasecmp(name, "esc")) kc = KEY_ESC;
	else if (!strcasecmp(name, "backspace")) kc = KEY_BACKSPACE;
	else if (!strcasecmp(name, "space")) kc = KEY_SPACE;
	else if (!strcasecmp(name, "up")) kc = KEY_UP;
	else if (!strcasecmp(name, "down")) kc = KEY_DOWN;
	else if (!strcasecmp(name, "left")) kc = KEY_LEFT;
	else if (!strcasecmp(name, "right")) kc = KEY_RIGHT;
	if (!kc) return;
	struct wlr_seat_client *sc = cua_kbd_enter(server, t);
	if (!sc) return;
	cua_kbd_key(sc, kc, true);
	cua_kbd_key(sc, kc, false);
}
/* ── control socket: one line per command, routed by app_id ───────────────── */
static void cua_handle_cmd(struct tinywl_server *server, char *line) {
	char cmd[8], app[128];
	if (sscanf(line, "%7s", cmd) != 1) return;
	if (!strcmp(cmd, "m")) {
		int idx; double x, y;
		if (sscanf(line, "m %127s %d %lf %lf", app, &idx, &x, &y) == 4)
			cua_motion(server, cua_find_appid(server, app), idx, x, y);
	} else if (!strcmp(cmd, "b")) {
		int idx; unsigned btn, pr;
		if (sscanf(line, "b %127s %d %u %u", app, &idx, &btn, &pr) == 4)
			cua_button(server, cua_find_appid(server, app), idx, btn, pr != 0);
	} else if (!strcmp(cmd, "t")) {
		char hex[8192];
		if (sscanf(line, "t %127s %8191s", app, hex) == 2)
			cua_type_hex(server, cua_find_appid(server, app), hex);
	} else if (!strcmp(cmd, "k")) {
		char key[32];
		if (sscanf(line, "k %127s %31s", app, key) == 2)
			cua_key_named(server, cua_find_appid(server, app), key);
	}
}
struct cua_conn { struct tinywl_server *server; struct wl_event_source *src; char buf[16384]; size_t len; };
/* Tear a connection down once: remove its event source (else the loop fires it
 * again on freed data -> double free), close the fd, free the state. */
static int cua_conn_drop(struct cua_conn *c, int fd) {
	if (c->src) wl_event_source_remove(c->src);
	close(fd); free(c);
	return 0;
}
static int cua_conn_readable(int fd, uint32_t mask, void *data) {
	struct cua_conn *c = data;
	if (mask & (WL_EVENT_HANGUP | WL_EVENT_ERROR)) return cua_conn_drop(c, fd);
	ssize_t n = read(fd, c->buf + c->len, sizeof(c->buf) - c->len - 1);
	if (n <= 0) return cua_conn_drop(c, fd);
	c->len += (size_t)n; c->buf[c->len] = 0;
	char *p = c->buf, *nl;
	while ((nl = memchr(p, '\n', (size_t)(c->buf + c->len - p)))) {
		*nl = 0; cua_handle_cmd(c->server, p); p = nl + 1;
	}
	size_t rem = (size_t)(c->buf + c->len - p);
	memmove(c->buf, p, rem); c->len = rem;
	wl_display_flush_clients(c->server->wl_display);
	return 0;
}
static int cua_listen_cb(int fd, uint32_t mask, void *data) {
	struct tinywl_server *server = data;
	int cfd = accept(fd, NULL, NULL);
	if (cfd < 0) return 0;
	struct cua_conn *c = calloc(1, sizeof *c); c->server = server;
	c->src = wl_event_loop_add_fd(wl_display_get_event_loop(server->wl_display), cfd,
		WL_EVENT_READABLE, cua_conn_readable, c);
	return 0;
}
static void cua_setup_control_socket(struct tinywl_server *server) {
	const char *path = getenv("CUA_INJECT_SOCKET");
	if (!path) { wlr_log(WLR_INFO, "[cua] no CUA_INJECT_SOCKET; injection disabled"); return; }
	int fd = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
	if (fd < 0) { wlr_log(WLR_ERROR, "[cua] socket(): %s", strerror(errno)); return; }
	struct sockaddr_un addr = {0}; addr.sun_family = AF_UNIX;
	snprintf(addr.sun_path, sizeof addr.sun_path, "%s", path);
	unlink(path);
	if (bind(fd, (struct sockaddr *)&addr, sizeof addr) < 0 || listen(fd, 4) < 0) {
		wlr_log(WLR_ERROR, "[cua] bind/listen %s: %s", path, strerror(errno)); close(fd); return;
	}
	cua_init_keymap();
	wl_event_loop_add_fd(wl_display_get_event_loop(server->wl_display), fd,
		WL_EVENT_READABLE, cua_listen_cb, server);
	wlr_log(WLR_INFO, "[cua] injection control socket on %s", path);
}

"""

def repl(s, old, new, label, count=1):
    if s.count(old) != count:
        sys.stderr.write("ANCHOR FAIL [%s]: found %d (want %d)\n" % (label, s.count(old), count))
        sys.exit(1)
    return s.replace(old, new, count)

# 1) includes + globals + a foreign-toplevel handle field on the toplevel.
src = repl(src, "struct tinywl_server {", INCLUDES + "struct tinywl_server {", "includes")
src = repl(src,
    "\tstruct wlr_xdg_toplevel *xdg_toplevel;\n",
    STRUCT_FIELD, "ftl-field")

# 2) injection helpers + control socket, just before main().
src = repl(src, "int main(int argc, char *argv[]) {", FUNCS + "int main(int argc, char *argv[]) {", "funcs")

# 3) On map: register a foreign-toplevel handle (title/app_id) for list_windows.
src = repl(src,
    "\twl_list_insert(&toplevel->server->toplevels, &toplevel->link);\n\n\tfocus_toplevel(toplevel);",
    "\twl_list_insert(&toplevel->server->toplevels, &toplevel->link);\n"
    "\tif (g_ftl_mgr) {\n"
    "\t\ttoplevel->ftl = wlr_foreign_toplevel_handle_v1_create(g_ftl_mgr);\n"
    "\t\tif (toplevel->xdg_toplevel->title)\n"
    "\t\t\twlr_foreign_toplevel_handle_v1_set_title(toplevel->ftl, toplevel->xdg_toplevel->title);\n"
    "\t\tif (toplevel->xdg_toplevel->app_id)\n"
    "\t\t\twlr_foreign_toplevel_handle_v1_set_app_id(toplevel->ftl, toplevel->xdg_toplevel->app_id);\n"
    "\t}\n\n\tfocus_toplevel(toplevel);",
    "ftl-on-map")

# 4) On unmap: drop the foreign-toplevel handle.
src = repl(src,
    "\twl_list_remove(&toplevel->link);\n}",
    "\tif (toplevel->ftl) { wlr_foreign_toplevel_handle_v1_destroy(toplevel->ftl); toplevel->ftl = NULL; }\n"
    "\twl_list_remove(&toplevel->link);\n}",
    "ftl-on-unmap")

# 5) On commit: keep title/app_id fresh (they often arrive after the map).
src = repl(src,
    "\t\twlr_xdg_toplevel_set_size(toplevel->xdg_toplevel, 0, 0);\n\t}",
    "\t\twlr_xdg_toplevel_set_size(toplevel->xdg_toplevel, 0, 0);\n\t}\n"
    "\tif (toplevel->ftl) {\n"
    "\t\tif (toplevel->xdg_toplevel->title)\n"
    "\t\t\twlr_foreign_toplevel_handle_v1_set_title(toplevel->ftl, toplevel->xdg_toplevel->title);\n"
    "\t\tif (toplevel->xdg_toplevel->app_id)\n"
    "\t\t\twlr_foreign_toplevel_handle_v1_set_app_id(toplevel->ftl, toplevel->xdg_toplevel->app_id);\n"
    "\t}",
    "ftl-on-commit")

# 6) Headless backend has no preferred mode -> set a real custom mode so the
#    scene has a non-zero output for screencopy/grim.
src = repl(src,
    "\tstruct wlr_output_mode *mode = wlr_output_preferred_mode(wlr_output);\n\tif (mode != NULL) {\n\t\twlr_output_state_set_mode(&state, mode);\n\t}",
    "\tstruct wlr_output_mode *mode = wlr_output_preferred_mode(wlr_output);\n\tif (mode != NULL) {\n\t\twlr_output_state_set_mode(&state, mode);\n\t} else {\n"
    "\t\tint ow = getenv(\"CUA_OUTW\") ? atoi(getenv(\"CUA_OUTW\")) : 1280;\n"
    "\t\tint oh = getenv(\"CUA_OUTH\") ? atoi(getenv(\"CUA_OUTH\")) : 1024;\n"
    "\t\twlr_output_state_set_custom_mode(&state, ow, oh, 0);\n\t}",
    "headless-custom-mode")

# 7) Create the extra managers + the seat caps + the control socket in main.
src = repl(src,
    "\tserver.output_layout = wlr_output_layout_create(server.wl_display);",
    "\tserver.output_layout = wlr_output_layout_create(server.wl_display);\n"
    "\twlr_xdg_output_manager_v1_create(server.wl_display, server.output_layout);\n"
    "\twlr_screencopy_manager_v1_create(server.wl_display);\n"
    "\tg_ftl_mgr = wlr_foreign_toplevel_manager_v1_create(server.wl_display);",
    "managers")
src = repl(src,
    'server.seat = wlr_seat_create(server.wl_display, "seat0");',
    'server.seat = wlr_seat_create(server.wl_display, "seat0");\n'
    "\twlr_seat_set_capabilities(server.seat, WL_SEAT_CAPABILITY_POINTER | WL_SEAT_CAPABILITY_KEYBOARD);",
    "seat-caps")
src = repl(src,
    "\twl_display_run(server.wl_display);",
    "\tcua_setup_control_socket(&server);\n\twl_display_run(server.wl_display);",
    "control-socket-setup")

io.open(out, "w", encoding="utf-8").write(src)
sys.stderr.write("cua-compositor.c written (%d bytes)\n" % len(src))
