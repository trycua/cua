#!/usr/bin/env python3
# Transforms wlroots' upstream `tinywl/tinywl.c` (0.19) into `cua-compositor.c`:
# a minimal headless wlroots compositor that cua-driver spawns as its nested
# session (CUA_WAYLAND_NEST_COMPOSITOR=cua-compositor) to perform what stock
# Wayland forbids a client from doing —
#
#   * focus-FREE per-surface KEYBOARD injection (type into an unfocused window),
#   * MULTI-cursor pointer injection (N independent cursors on one window),
#
# both routed by stable process identity (with app_id fallback), driven over a tiny line
# protocol on a unix control socket ($CUA_INJECT_SOCKET). It also exposes
# foreign-toplevel-management (so cua-driver's list_windows enumerates windows)
# and screencopy (so grim captures the output) — the same protocols labwc gives
# the non-nested cells — so the nested-injection cells keep working end to end.
#
# The injection primitives (cua_motion/cua_button/cua_kbd_*) deliver wl_pointer/
# wl_keyboard straight to a target client's resources, bypassing seat focus —
# routed by process identity rather than a connection-local object id and transported over a plain
# socket rather than libei/EIS, since cua owns both ends and the portal/libei
# layer buys nothing here. The socket speaks a versioned v1 line protocol: the
# client sends the `cua-inject v1` banner (echoed back on match), then every
# command line is answered by exactly one `ok` / `err <reason>` acknowledgement.
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
#include <strings.h>
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
static xkb_mod_mask_t g_ctrl_mask = 0;
static xkb_mod_mask_t g_alt_mask = 0;
static xkb_mod_mask_t g_logo_mask = 0;
struct cua_keyent { uint32_t keycode; int shift; int valid; };
static struct cua_keyent g_chartab[128];
static struct wlr_foreign_toplevel_manager_v1 *g_ftl_mgr = NULL;
static void cua_ftl_request_activate(struct wl_listener *listener, void *data);

"""

# A foreign-toplevel handle pointer on each toplevel (for list_windows).
STRUCT_FIELD = (
    "\tstruct wlr_xdg_toplevel *xdg_toplevel;\n"
    "\tstruct wlr_foreign_toplevel_handle_v1 *ftl;\n"
    "\tstruct wl_listener ftl_request_activate;\n"
)

FUNCS = r"""
/* v1 control-protocol banner: the client sends this line, the compositor echoes
 * it to confirm both speak v1. Any other first line is refused. */
#define CUA_PROTO_HELLO "cua-inject v1"
static void cua_ftl_request_activate(struct wl_listener *listener, void *data) {
	(void)data;
	struct tinywl_toplevel *t = wl_container_of(listener, t, ftl_request_activate);
	focus_toplevel(t);
}
static uint32_t cua_now_ms(void) {
	struct timespec ts; clock_gettime(CLOCK_MONOTONIC, &ts);
	return (uint32_t)(ts.tv_sec * 1000 + ts.tv_nsec / 1000000);
}
/* Write a single acknowledgement line back to the control client. Best-effort:
 * a dead peer is torn down by the read side on the next loop iteration. */
static void cua_reply(int fd, const char *line) {
	char buf[256];
	int n = snprintf(buf, sizeof buf, "%s\n", line);
	if (n < 0) return;
	if (n >= (int)sizeof buf) n = (int)sizeof buf - 1;
	ssize_t off = 0;
	while (off < n) {
		ssize_t w = write(fd, buf + off, (size_t)n - (size_t)off);
		if (w <= 0) break;
		off += w;
	}
}
static void cua_pframe(struct wl_resource *res) {
	if (wl_resource_get_version(res) >= WL_POINTER_FRAME_SINCE_VERSION)
		wl_pointer_send_frame(res);
}
static pid_t cua_toplevel_pid(struct tinywl_toplevel *t);
static bool cua_pid_in_family(pid_t pid, pid_t root_pid) {
	if (pid <= 0 || root_pid <= 0) return false;
	for (int depth = 0; depth < 64 && pid > 1; depth++) {
		if (pid == root_pid) return true;
		char path[64], stat[4096];
		snprintf(path, sizeof path, "/proc/%d/stat", (int)pid);
		FILE *file = fopen(path, "r");
		if (!file) return false;
		size_t n = fread(stat, 1, sizeof stat - 1, file);
		fclose(file);
		if (!n) return false;
		stat[n] = '\0';
		/* comm is parenthesized and may contain spaces or ')', so parse the
		 * state + parent pid after its final closing parenthesis. */
		char *close = strrchr(stat, ')'), state = '\0';
		long parent = 0;
		if (!close || sscanf(close + 2, "%c %ld", &state, &parent) != 2 ||
			parent <= 0 || parent == pid) return false;
		pid = (pid_t)parent;
	}
	return pid == root_pid;
}
/* Resolve a target window by its xdg app_id, refusing missing and ambiguous
 * matches so a command never silently drives the wrong window. In v1 duplicate
 * app_ids are simply not addressable. On failure returns NULL and points *err
 * at a stable reason token; on success *err is left untouched. */
static struct tinywl_toplevel *cua_resolve_target(struct tinywl_server *server, const char *app_id, const char **err) {
	struct tinywl_toplevel *t, *found = NULL;
	int matches = 0;
	if (!strncmp(app_id, "root:", 5)) {
		char *end = NULL;
		long pid = strtol(app_id + 5, &end, 10);
		if (pid <= 0 || !end || *end) { *err = "bad-root-pid"; return NULL; }
		wl_list_for_each(t, &server->toplevels, link) {
			if (cua_pid_in_family(cua_toplevel_pid(t), (pid_t)pid)) { found = t; matches++; }
		}
		if (matches == 0) { *err = "unknown-root-pid"; return NULL; }
		if (matches > 1) { *err = "ambiguous-root-pid"; return NULL; }
		return found;
	}
	if (!strncmp(app_id, "pid:", 4)) {
		char *end = NULL;
		long pid = strtol(app_id + 4, &end, 10);
		if (pid <= 0 || !end || *end) { *err = "bad-pid"; return NULL; }
		wl_list_for_each(t, &server->toplevels, link) {
			if (cua_toplevel_pid(t) == (pid_t)pid) { found = t; matches++; }
		}
		if (matches == 0) { *err = "unknown-pid"; return NULL; }
		if (matches > 1) { *err = "ambiguous-pid"; return NULL; }
		return found;
	}
	wl_list_for_each(t, &server->toplevels, link) {
		const char *a = t->xdg_toplevel ? t->xdg_toplevel->app_id : NULL;
		if (a && strcmp(a, app_id) == 0) { found = t; matches++; }
	}
	if (matches == 0) { *err = "unknown-app-id"; return NULL; }
	if (matches > 1) { *err = "ambiguous-app-id"; return NULL; }
	return found;
}
static pid_t cua_toplevel_pid(struct tinywl_toplevel *t) {
	if (!t || !t->xdg_toplevel || !t->xdg_toplevel->base->surface) return 0;
	struct wl_client *client = wl_resource_get_client(t->xdg_toplevel->base->surface->resource);
	pid_t pid = 0; uid_t uid = 0; gid_t gid = 0;
	wl_client_get_credentials(client, &pid, &uid, &gid);
	return pid;
}
/* Focus exactly one mapped toplevel owned by `target_pid`. Refuse ambiguity:
 * process-scoped activation is only safe when the process owns one window. */
static const char *cua_activate_pid(struct tinywl_server *server, pid_t target_pid) {
	struct tinywl_toplevel *t, *found = NULL;
	int matches = 0;
	wl_list_for_each(t, &server->toplevels, link) {
		if (target_pid > 0 && cua_toplevel_pid(t) == target_pid) {
			found = t;
			matches++;
		}
	}
	if (matches == 0) return "unknown-pid";
	if (matches > 1) return "ambiguous-pid";
	focus_toplevel(found);
	return NULL;
}
/* Independent observer query used only by the Rust E2E testkit. The target is
 * selected by the Wayland client's process credentials, not by driver-owned
 * object ids. In this minimal compositor every mapped toplevel shares origin;
 * a non-focused target beneath another focused surface is therefore occluded. */
static void cua_query_state(struct tinywl_server *server, pid_t target_pid, char *out, size_t out_len) {
	struct wlr_surface *focused = server->seat->keyboard_state.focused_surface;
	struct wlr_surface *focused_root = focused ? wlr_surface_get_root_surface(focused) : NULL;
	struct tinywl_toplevel *t, *target = NULL, *focused_toplevel = NULL;
	wl_list_for_each(t, &server->toplevels, link) {
		struct wlr_surface *surface = t->xdg_toplevel ? t->xdg_toplevel->base->surface : NULL;
		if (surface == focused_root) focused_toplevel = t;
		if (target_pid > 0 && cua_toplevel_pid(t) == target_pid) target = t;
	}
	pid_t focused_pid = cua_toplevel_pid(focused_toplevel);
	const char *state = !target ? "not_found" :
		(target == focused_toplevel ? "foreground" :
		 (focused_toplevel ? "background_occluded" : "background_visible"));
	snprintf(out, out_len, "state %d %s", (int)focused_pid, state);
}
static const char *cua_query_geometry(struct tinywl_server *server, pid_t target_pid, char *out, size_t out_len) {
	struct tinywl_toplevel *t, *target = NULL;
	int matches = 0;
	wl_list_for_each(t, &server->toplevels, link) {
		if (cua_toplevel_pid(t) == target_pid) { target = t; matches++; }
	}
	if (!matches) return "target-not-found";
	if (matches > 1) return "ambiguous-pid";
	int x = 0, y = 0;
	if (!wlr_scene_node_coords(&target->scene_tree->node, &x, &y)) return "unmapped-target";
	/* AT-SPI Window coordinates for native GTK are rooted at the xdg window
	 * geometry, while scene coordinates and screencopy include the complete root
	 * surface (including client-side decorations). Rebase into that coordinate
	 * system so `origin + accessible_window_xy` lands on the captured pixel. */
	int scene_x = x, scene_y = y;
	struct wlr_box geo = target->xdg_toplevel->base->geometry;
	x -= geo.x; y -= geo.y;
	snprintf(out, out_len, "geometry %d %d %d %d", x, y, scene_x, scene_y);
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
static bool cua_motion(struct tinywl_server *server, struct tinywl_toplevel *t, int idx, double x, double y) {
	if (!t || idx < 0 || idx >= CUA_MAXDEV) return false;
	/* Public PX coordinates come from the cropped root-surface screenshot. Hit
	 * test that point through the scene so Chromium/WebKit child surfaces receive
	 * enter/motion in their own local coordinates instead of the top-level root. */
	int scene_x = 0, scene_y = 0;
	if (!wlr_scene_node_coords(&t->scene_tree->node, &scene_x, &scene_y)) return false;
	double local_x = 0, local_y = 0;
	/* Search only the requested toplevel's scene subtree. A global hit test
	 * would select the foreground sentinel when this target is occluded. */
	struct wlr_scene_node *node = wlr_scene_node_at(&t->scene_tree->node,
		scene_x + x, scene_y + y, &local_x, &local_y);
	if (!node || node->type != WLR_SCENE_NODE_BUFFER) return false;
	struct wlr_scene_buffer *buffer = wlr_scene_buffer_from_node(node);
	struct wlr_scene_surface *scene_surface = wlr_scene_surface_try_from_buffer(buffer);
	if (!scene_surface) return false;
	struct wlr_surface *surface = scene_surface->surface;
	struct wlr_seat_client *sc = wlr_seat_client_for_wl_client(server->seat, wl_resource_get_client(surface->resource));
	if (!sc || wl_list_empty(&sc->pointers)) {
		/* Chromium can compose a renderer-owned child surface whose wl_client
		 * never bound wl_pointer while the owning toplevel client did. Use that
		 * target root while retaining the caller's screenshot-local point. */
		surface = t->xdg_toplevel->base->surface;
		local_x = x; local_y = y;
		sc = wlr_seat_client_for_wl_client(server->seat, wl_resource_get_client(surface->resource));
		if (!sc || wl_list_empty(&sc->pointers)) return false;
	}
	wl_fixed_t sx = wl_fixed_from_double(local_x), sy = wl_fixed_from_double(local_y);
	struct wl_resource *res;
	if (cua_ptr[idx].entered != surface) {
		if (cua_ptr[idx].entered) cua_ptr_leave(server->seat, cua_ptr[idx].entered);
		uint32_t es = wlr_seat_client_next_serial(sc);
		wl_resource_for_each(res, &sc->pointers) { wl_pointer_send_enter(res, es, surface->resource, sx, sy); cua_pframe(res); }
		cua_ptr[idx].entered = surface;
	}
	uint32_t tm = cua_now_ms();
	wl_resource_for_each(res, &sc->pointers) { wl_pointer_send_motion(res, tm, sx, sy); cua_pframe(res); }
	return true;
}
/* Resolve an output-layout point through the compositor scene, preserving
 * subsurface offsets and output scaling. This is the desktop-scope path. */
static struct tinywl_toplevel *cua_desktop_motion(struct tinywl_server *server, double x, double y) {
	double sx = 0, sy = 0;
	struct wlr_surface *surface = NULL;
	struct tinywl_toplevel *t = desktop_toplevel_at(server, x, y, &surface, &sx, &sy);
	if (!t || !surface) return NULL;
	struct wlr_seat_client *sc = wlr_seat_client_for_wl_client(server->seat, wl_resource_get_client(surface->resource));
	if (!sc || wl_list_empty(&sc->pointers)) return NULL;
	struct wl_resource *res;
	if (cua_ptr[0].entered != surface) {
		if (cua_ptr[0].entered) cua_ptr_leave(server->seat, cua_ptr[0].entered);
		uint32_t serial = wlr_seat_client_next_serial(sc);
		wl_resource_for_each(res, &sc->pointers) {
			wl_pointer_send_enter(res, serial, surface->resource, wl_fixed_from_double(sx), wl_fixed_from_double(sy));
			cua_pframe(res);
		}
		cua_ptr[0].entered = surface;
	}
	uint32_t tm = cua_now_ms();
	wl_resource_for_each(res, &sc->pointers) {
		wl_pointer_send_motion(res, tm, wl_fixed_from_double(sx), wl_fixed_from_double(sy));
		cua_pframe(res);
	}
	return t;
}
static bool cua_button(struct tinywl_server *server, struct tinywl_toplevel *t, int idx, uint32_t button, bool pressed) {
	if (!t || idx < 0 || idx >= CUA_MAXDEV) return false;
	/* `cua_motion` establishes the exact child or root surface for this logical
	 * pointer. Button and axis events must use that same wl_pointer resource. */
	struct wlr_surface *surface = cua_ptr[idx].entered ? cua_ptr[idx].entered : t->xdg_toplevel->base->surface;
	struct wlr_seat_client *sc = wlr_seat_client_for_wl_client(server->seat, wl_resource_get_client(surface->resource));
	if (!sc || wl_list_empty(&sc->pointers)) return false;
	uint32_t tm = cua_now_ms(), bs = wlr_seat_client_next_serial(sc);
	struct wl_resource *res;
	wl_resource_for_each(res, &sc->pointers) {
		wl_pointer_send_button(res, bs, tm, button, pressed ? WL_POINTER_BUTTON_STATE_PRESSED : WL_POINTER_BUTTON_STATE_RELEASED);
		cua_pframe(res);
	}
	return true;
}
static bool cua_axis(struct tinywl_server *server, struct tinywl_toplevel *t, int idx, uint32_t axis, double value) {
	if (!t || idx < 0 || idx >= CUA_MAXDEV) return false;
	struct wlr_surface *surface = cua_ptr[idx].entered ? cua_ptr[idx].entered : t->xdg_toplevel->base->surface;
	struct wlr_seat_client *sc = wlr_seat_client_for_wl_client(server->seat, wl_resource_get_client(surface->resource));
	if (!sc || wl_list_empty(&sc->pointers)) return false;
	uint32_t tm = cua_now_ms();
	struct wl_resource *res;
	wl_resource_for_each(res, &sc->pointers) {
		int32_t step = value < 0 ? -1 : 1;
		if (wl_resource_get_version(res) >= WL_POINTER_AXIS_SOURCE_SINCE_VERSION)
			wl_pointer_send_axis_source(res, WL_POINTER_AXIS_SOURCE_WHEEL);
		if (wl_resource_get_version(res) >= WL_POINTER_AXIS_VALUE120_SINCE_VERSION)
			wl_pointer_send_axis_value120(res, axis, step * 120);
		else if (wl_resource_get_version(res) >= WL_POINTER_AXIS_DISCRETE_SINCE_VERSION)
			wl_pointer_send_axis_discrete(res, axis, step);
		wl_pointer_send_axis(res, tm, axis, wl_fixed_from_double(value));
		cua_pframe(res);
	}
	return true;
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
	xkb_mod_index_t ctrl = xkb_keymap_mod_get_index(km, XKB_MOD_NAME_CTRL);
	if (ctrl != XKB_MOD_INVALID) g_ctrl_mask = (xkb_mod_mask_t)1 << ctrl;
	xkb_mod_index_t alt = xkb_keymap_mod_get_index(km, XKB_MOD_NAME_ALT);
	if (alt != XKB_MOD_INVALID) g_alt_mask = (xkb_mod_mask_t)1 << alt;
	xkb_mod_index_t logo = xkb_keymap_mod_get_index(km, XKB_MOD_NAME_LOGO);
	if (logo != XKB_MOD_INVALID) g_logo_mask = (xkb_mod_mask_t)1 << logo;
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
/* Preserve compositor-native keyboard delivery when the addressed surface is
 * already the logical seat focus. Chromium relies on wlroots' focused-seat
 * state for foreground keyboard events; sending a raw wl_keyboard.key to its
 * resource can be acknowledged while never reaching the renderer. Background
 * targets retain the direct resource path that makes focus-free input possible. */
static void cua_kbd_key_target(struct tinywl_server *server, struct tinywl_toplevel *t,
		struct wlr_seat_client *sc, uint32_t keycode, bool pressed) {
	struct wlr_surface *target = wlr_surface_get_root_surface(t->xdg_toplevel->base->surface);
	struct wlr_surface *focused = server->seat->keyboard_state.focused_surface;
	struct wlr_surface *focused_root = focused ? wlr_surface_get_root_surface(focused) : NULL;
	if (target == focused_root) {
		wlr_seat_keyboard_notify_key(server->seat, cua_now_ms(), keycode,
			pressed ? WL_KEYBOARD_KEY_STATE_PRESSED : WL_KEYBOARD_KEY_STATE_RELEASED);
	} else {
		cua_kbd_key(sc, keycode, pressed);
	}
}
static bool cua_type_cp(struct tinywl_server *server, struct tinywl_toplevel *t, uint32_t cp) {
	if (cp >= 128 || !g_chartab[cp].valid) return false;
	struct wlr_seat_client *sc = cua_kbd_enter(server, t);
	if (!sc) return false;
	struct cua_keyent e = g_chartab[cp];
	if (e.shift) cua_kbd_mods(sc, g_shift_mask);
	cua_kbd_key_target(server, t, sc, e.keycode, true);
	cua_kbd_key_target(server, t, sc, e.keycode, false);
	if (e.shift) cua_kbd_mods(sc, 0);
	return true;
}
/* Decode a hex-encoded ASCII string and type it focus-free into `t`. */
static bool cua_type_hex(struct tinywl_server *server, struct tinywl_toplevel *t, const char *hex) {
	if (!t) return false;
	for (const char *p = hex; p[0] && p[1]; p += 2) {
		int hi = (p[0] <= '9') ? p[0] - '0' : (p[0] | 0x20) - 'a' + 10;
		int lo = (p[1] <= '9') ? p[1] - '0' : (p[1] | 0x20) - 'a' + 10;
		if (!cua_type_cp(server, t, (uint32_t)((hi << 4) | lo))) return false;
	}
	return true;
}
static uint32_t cua_named_keycode(const char *name) {
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
	else if (!strncasecmp(name, "f", 1)) {
		char *end = NULL; long fn = strtol(name + 1, &end, 10);
		if (end && !*end && fn >= 1 && fn <= 10) kc = KEY_F1 + (uint32_t)fn - 1;
		else if (end && !*end && fn == 11) kc = KEY_F11;
		else if (end && !*end && fn == 12) kc = KEY_F12;
	}
	return kc;
}
/* Returns 1 when `name` is a recognised key (delivered if the target had a
 * keyboard bound), 0 when it is outside the whitelist so the caller can NAK. */
static int cua_key_named(struct tinywl_server *server, struct tinywl_toplevel *t, const char *name) {
	if (!t) return 0;
	uint32_t kc = cua_named_keycode(name);
	if (!kc) return 0;
	struct wlr_seat_client *sc = cua_kbd_enter(server, t);
	if (!sc) return -1;
	cua_kbd_key_target(server, t, sc, kc, true);
	cua_kbd_key_target(server, t, sc, kc, false);
	return 1;
}
static int cua_hotkey(struct tinywl_server *server, struct tinywl_toplevel *t, const char *mods, const char *key) {
	if (!t) return 0;
	uint32_t kc = cua_named_keycode(key);
	if (!kc && key[0] && !key[1]) {
		unsigned char cp = (unsigned char)key[0];
		if (cp < 128 && g_chartab[cp].valid) kc = g_chartab[cp].keycode;
	}
	if (!kc) return 0;
	xkb_mod_mask_t mask = 0;
	char copy[128]; snprintf(copy, sizeof copy, "%s", mods);
	char *save = NULL;
	for (char *mod = strtok_r(copy, ",", &save); mod; mod = strtok_r(NULL, ",", &save)) {
		if (!strcasecmp(mod, "ctrl") || !strcasecmp(mod, "control")) mask |= g_ctrl_mask;
		else if (!strcasecmp(mod, "shift")) mask |= g_shift_mask;
		else if (!strcasecmp(mod, "alt") || !strcasecmp(mod, "option")) mask |= g_alt_mask;
		else if (!strcasecmp(mod, "meta") || !strcasecmp(mod, "super") || !strcasecmp(mod, "win") || !strcasecmp(mod, "cmd")) mask |= g_logo_mask;
		else return 0;
	}
	struct wlr_seat_client *sc = cua_kbd_enter(server, t);
	if (!sc) return -1;
	cua_kbd_mods(sc, mask);
	cua_kbd_key_target(server, t, sc, kc, true);
	cua_kbd_key_target(server, t, sc, kc, false);
	cua_kbd_mods(sc, 0);
	return 1;
}
/* ── control socket: one line per command, routed by app_id ───────────────── */
/* Process one command line. Returns NULL on success, else a stable error token
 * the caller sends back as `err <token>`. The command is only acknowledged
 * after it has been resolved and applied — never before. */
static const char *cua_handle_cmd(struct tinywl_server *server, char *line) {
	char cmd[8], app[128];
	if (sscanf(line, "%7s", cmd) != 1) return "empty";
	const char *err = NULL;
	struct tinywl_toplevel *t;
	if (!strcmp(cmd, "d")) {
		double x, y; unsigned count, btn;
		if (sscanf(line, "d %lf %lf %u %u", &x, &y, &count, &btn) != 4) return "bad-args";
		if (!(t = cua_desktop_motion(server, x, y))) return "no-surface-at-point";
		for (unsigned i = 0; i < (count ? count : 1); i++) {
			if (!cua_button(server, t, 0, btn, true)) return "no-pointer-resource";
			if (!cua_button(server, t, 0, btn, false)) return "no-pointer-resource";
		}
		return NULL;
	} else if (!strcmp(cmd, "m")) {
		int idx; double x, y;
		if (sscanf(line, "m %127s %d %lf %lf", app, &idx, &x, &y) != 4) return "bad-args";
		if (!(t = cua_resolve_target(server, app, &err))) return err;
		if (!cua_motion(server, t, idx, x, y)) return "no-pointer-resource";
		return NULL;
	} else if (!strcmp(cmd, "b")) {
		int idx; unsigned btn, pr;
		if (sscanf(line, "b %127s %d %u %u", app, &idx, &btn, &pr) != 4) return "bad-args";
		if (!(t = cua_resolve_target(server, app, &err))) return err;
		if (!cua_button(server, t, idx, btn, pr != 0)) return "no-pointer-resource";
		return NULL;
	} else if (!strcmp(cmd, "t")) {
		char hex[8192];
		if (sscanf(line, "t %127s %8191s", app, hex) != 2) return "bad-args";
		if (!(t = cua_resolve_target(server, app, &err))) return err;
		if (!cua_type_hex(server, t, hex)) return "no-keyboard-resource";
		return NULL;
	} else if (!strcmp(cmd, "k")) {
		char key[32];
		if (sscanf(line, "k %127s %31s", app, key) != 2) return "bad-args";
		if (!(t = cua_resolve_target(server, app, &err))) return err;
		int result = cua_key_named(server, t, key);
		if (result < 0) return "no-keyboard-resource";
		if (!result) return "unknown-key";
		return NULL;
	} else if (!strcmp(cmd, "h")) {
		char mods[128], key[32];
		if (sscanf(line, "h %127s %127s %31s", app, mods, key) != 3) return "bad-args";
		if (!(t = cua_resolve_target(server, app, &err))) return err;
		int result = cua_hotkey(server, t, mods, key);
		if (result < 0) return "no-keyboard-resource";
		if (!result) return "unknown-hotkey";
		return NULL;
	} else if (!strcmp(cmd, "a")) {
		int idx; unsigned axis; double value;
		if (sscanf(line, "a %127s %d %u %lf", app, &idx, &axis, &value) != 4) return "bad-args";
		if (!(t = cua_resolve_target(server, app, &err))) return err;
		if (!cua_axis(server, t, idx, axis, value)) return "no-pointer-resource";
		return NULL;
	}
	return "unknown-command";
}
struct cua_conn { struct tinywl_server *server; struct wl_event_source *src; int hello; char buf[16384]; size_t len; };
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
		*nl = 0;
		/* Tolerate CRLF clients by trimming a trailing carriage return. */
		if (nl > p && nl[-1] == '\r') nl[-1] = 0;
		if (!c->hello) {
			/* The first line must be the versioned v1 handshake. */
			if (!strcmp(p, CUA_PROTO_HELLO)) {
				c->hello = 1;
				cua_reply(fd, CUA_PROTO_HELLO);
			} else {
				cua_reply(fd, "err unsupported-version");
				return cua_conn_drop(c, fd);
			}
		} else {
			int query_pid, geometry_pid, activate_pid;
			if (sscanf(p, "q %d", &query_pid) == 1) {
				char msg[128]; cua_query_state(c->server, (pid_t)query_pid, msg, sizeof msg);
				cua_reply(fd, msg);
			} else if (sscanf(p, "g %d", &geometry_pid) == 1) {
				char msg[128];
				const char *err = cua_query_geometry(c->server, (pid_t)geometry_pid, msg, sizeof msg);
				if (err) {
					char reply[128]; snprintf(reply, sizeof reply, "err %s", err); cua_reply(fd, reply);
				} else {
					cua_reply(fd, msg);
				}
			} else if (sscanf(p, "f %d", &activate_pid) == 1) {
				const char *err = cua_activate_pid(c->server, (pid_t)activate_pid);
				wl_display_flush_clients(c->server->wl_display);
				if (err) {
					char msg[128]; snprintf(msg, sizeof msg, "err %s", err); cua_reply(fd, msg);
				} else {
					cua_reply(fd, "ok");
				}
			} else {
				const char *err = cua_handle_cmd(c->server, p);
				/* Deliver injected events before acking so `ok` means "processed",
				 * never merely "parsed". */
				wl_display_flush_clients(c->server->wl_display);
				if (err) {
					char msg[128];
					snprintf(msg, sizeof msg, "err %s", err);
					cua_reply(fd, msg);
				} else {
					cua_reply(fd, "ok");
				}
			}
		}
		p = nl + 1;
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
# tinywl's desktop-oriented focus helper sends xdg_toplevel activation
# configures on every focus transition. Chromium can stop scheduling renderer
# frames after that configure in this minimal headless compositor. The private
# compositor uses seat focus plus scene stacking as its source of truth, so keep
# those semantics without advertising xdg-shell activation state.
src = repl(src,
    "\tif (prev_surface) {\n"
    "\t\t/*\n"
    "\t\t * Deactivate the previously focused surface. This lets the client know\n"
    "\t\t * it no longer has focus and the client will repaint accordingly, e.g.\n"
    "\t\t * stop displaying a caret.\n"
    "\t\t */\n"
    "\t\tstruct wlr_xdg_toplevel *prev_toplevel =\n"
    "\t\t\twlr_xdg_toplevel_try_from_wlr_surface(prev_surface);\n"
    "\t\tif (prev_toplevel != NULL) {\n"
    "\t\t\twlr_xdg_toplevel_set_activated(prev_toplevel, false);\n"
    "\t\t}\n"
    "\t}\n",
    "", "headless-skip-deactivation")
src = repl(src,
    "\t/* Activate the new surface */\n"
    "\twlr_xdg_toplevel_set_activated(toplevel->xdg_toplevel, true);\n",
    "\t/* Seat focus and scene stacking are authoritative in the private\n"
    "\t * headless compositor; avoid an xdg activation configure here. */\n",
    "headless-skip-activation")
src = repl(src,
    "\tif (keyboard != NULL) {\n"
    "\t\twlr_seat_keyboard_notify_enter(seat, surface,\n"
    "\t\t\tkeyboard->keycodes, keyboard->num_keycodes, &keyboard->modifiers);\n"
    "\t}\n",
    "\tif (keyboard != NULL) {\n"
    "\t\twlr_seat_keyboard_notify_enter(seat, surface,\n"
    "\t\t\tkeyboard->keycodes, keyboard->num_keycodes, &keyboard->modifiers);\n"
    "\t} else {\n"
    "\t\tstruct wlr_keyboard_modifiers modifiers = {0};\n"
    "\t\twlr_seat_keyboard_notify_enter(seat, surface, NULL, 0, &modifiers);\n"
    "\t}\n",
    "headless-seat-focus")
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
    "\t\ttoplevel->ftl_request_activate.notify = cua_ftl_request_activate;\n"
    "\t\twl_signal_add(&toplevel->ftl->events.request_activate, &toplevel->ftl_request_activate);\n"
    "\t}\n\n\tfocus_toplevel(toplevel);",
    "ftl-on-map")

# 4) On unmap: drop the foreign-toplevel handle.
src = repl(src,
    "\twl_list_remove(&toplevel->link);\n}",
    "\tstruct wlr_surface *cua_surface = toplevel->xdg_toplevel->base->surface;\n"
    "\tfor (int i = 0; i < CUA_MAXDEV; i++) {\n"
    "\t\tif (cua_ptr[i].entered && wlr_surface_get_root_surface(cua_ptr[i].entered) == cua_surface) cua_ptr[i].entered = NULL;\n"
    "\t\tif (cua_kbd_state[i].entered && wlr_surface_get_root_surface(cua_kbd_state[i].entered) == cua_surface) cua_kbd_state[i].entered = NULL;\n"
    "\t}\n"
    "\tif (toplevel->ftl) { wl_list_remove(&toplevel->ftl_request_activate.link); wlr_foreign_toplevel_handle_v1_destroy(toplevel->ftl); toplevel->ftl = NULL; }\n"
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
