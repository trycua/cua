#!/usr/bin/env python3
# Phase E: add KEYBOARD injection to the EIS-server compositor (on top of the
# Phase D pointer routing + screencopy/xdg-output/headless-mode/grid).
# Device 0..g_nkbd-1 are virtual keyboards; the rest are abs pointers. Each
# device idx routes to cua_win[idx] (keyboard -> wl_keyboard, pointer -> wl_pointer).
import sys, io
P = "/opt/spike/tinywl.c"
src = io.open(P, encoding="utf-8").read()

INCLUDES = """#include <stdint.h>
#include <string.h>
#include <sys/mman.h>
#include <cairo/cairo.h>
#include <linux/input-event-codes.h>
#include <wayland-server-protocol.h>
#include <libeis.h>
#include <wlr/types/wlr_screencopy_v1.h>
#include <wlr/types/wlr_xdg_output_v1.h>
#include <wlr/interfaces/wlr_buffer.h>

#define CUA_MAXDEV 64
static struct eis *g_eis = NULL;
static int g_ndev = 16;
static int g_nkbd = 0;
static int g_keymap_fd = -1;
static size_t g_keymap_size = 0;
struct cua_devstate { struct wlr_surface *entered; struct wlr_scene_buffer *cursor; };
static struct cua_devstate cua_dev[CUA_MAXDEV];
static struct cua_devstate cua_kbd[CUA_MAXDEV];
static struct tinywl_toplevel *cua_win[CUA_MAXDEV];

"""

FUNCS = r"""
static uint32_t cua_now_ms(void) {
	struct timespec ts; clock_gettime(CLOCK_MONOTONIC, &ts);
	return (uint32_t)(ts.tv_sec * 1000 + ts.tv_nsec / 1000000);
}
static void cua_frame(struct wl_resource *res) {
	if (wl_resource_get_version(res) >= WL_POINTER_FRAME_SINCE_VERSION)
		wl_pointer_send_frame(res);
}
#define CUA_FOURCC_ARGB8888 0x34325241
struct cua_cbuf { struct wlr_buffer base; cairo_surface_t *surf; };
static void cua_cbuf_destroy(struct wlr_buffer *b) { struct cua_cbuf *c = wl_container_of(b, c, base); cairo_surface_destroy(c->surf); free(c); }
static bool cua_cbuf_begin(struct wlr_buffer *b, uint32_t flags, void **data, uint32_t *format, size_t *stride) {
	struct cua_cbuf *c = wl_container_of(b, c, base);
	*data = cairo_image_surface_get_data(c->surf);
	*stride = cairo_image_surface_get_stride(c->surf);
	*format = CUA_FOURCC_ARGB8888; return true;
}
static void cua_cbuf_end(struct wlr_buffer *b) {}
static const struct wlr_buffer_impl cua_cbuf_impl = { .destroy = cua_cbuf_destroy, .begin_data_ptr_access = cua_cbuf_begin, .end_data_ptr_access = cua_cbuf_end };
static struct wlr_buffer *cua_arrow_buffer(float r, float g, float b) {
	int W = 34, H = 44;
	cairo_surface_t *s = cairo_image_surface_create(CAIRO_FORMAT_ARGB32, W, H);
	cairo_t *cr = cairo_create(s);
	/* classic left-pointer outline (tip at top-left), scaled; matches cua's arrow look */
	double sc = 1.6, ax = 3, ay = 3;
	double p[7][2] = {{0,0},{0,18},{4.5,13.8},{7.6,20.6},{10.3,19.4},{7.0,12.7},{13.4,12.7}};
	cairo_move_to(cr, ax + p[0][0]*sc, ay + p[0][1]*sc);
	for (int i = 1; i < 7; i++) cairo_line_to(cr, ax + p[i][0]*sc, ay + p[i][1]*sc);
	cairo_close_path(cr);
	cairo_set_line_join(cr, CAIRO_LINE_JOIN_ROUND);
	cairo_set_source_rgba(cr, 1, 1, 1, 0.97); cairo_set_line_width(cr, 3.4); cairo_stroke_preserve(cr);
	cairo_set_source_rgba(cr, r, g, b, 1.0); cairo_fill(cr);
	cairo_destroy(cr); cairo_surface_flush(s);
	struct cua_cbuf *cb = calloc(1, sizeof *cb);
	wlr_buffer_init(&cb->base, &cua_cbuf_impl, W, H);
	cb->surf = s; return &cb->base;
}
static void cua_hsv(double h, double s, double v, float *r, float *g, float *b) {
	h -= (int)h; int i = (int)(h*6); double f = h*6-i, p=v*(1-s), q=v*(1-f*s), t=v*(1-(1-f)*s);
	switch (i%6) { case 0:*r=v;*g=t;*b=p;break; case 1:*r=q;*g=v;*b=p;break; case 2:*r=p;*g=v;*b=t;break;
		case 3:*r=p;*g=q;*b=v;break; case 4:*r=t;*g=p;*b=v;break; default:*r=v;*g=p;*b=q; }
}
static void cua_cursor_at(struct tinywl_server *server, int idx, int ox, int oy) {
	if (!cua_dev[idx].cursor) {
		float r, g, b; cua_hsv(idx*0.6180339887, 0.85, 0.98, &r, &g, &b);
		struct wlr_buffer *buf = cua_arrow_buffer(r, g, b);
		cua_dev[idx].cursor = wlr_scene_buffer_create(&server->scene->tree, buf);
		wlr_buffer_drop(buf);
	}
	wlr_scene_node_set_position(&cua_dev[idx].cursor->node, ox - 3, oy - 3);
	wlr_scene_node_raise_to_top(&cua_dev[idx].cursor->node);
}
static void cua_motion(struct tinywl_server *server, int idx, double x, double y) {
	struct tinywl_toplevel *t = (idx >= 0 && idx < CUA_MAXDEV) ? cua_win[idx] : NULL;
	if (!t) return;
	struct wlr_surface *surface = t->xdg_toplevel->base->surface;
	struct wlr_box geo = t->xdg_toplevel->base->geometry;
	wl_fixed_t sx = wl_fixed_from_double(geo.x + x), sy = wl_fixed_from_double(geo.y + y);
	struct wlr_seat_client *sc = wlr_seat_client_for_wl_client(server->seat, wl_resource_get_client(surface->resource));
	if (!sc || wl_list_empty(&sc->pointers)) return;
	cua_cursor_at(server, idx, t->scene_tree->node.x + geo.x + (int)x, t->scene_tree->node.y + geo.y + (int)y);
	struct wl_resource *res;
	if (cua_dev[idx].entered != surface) {
		if (cua_dev[idx].entered) {
			struct wlr_seat_client *o = wlr_seat_client_for_wl_client(server->seat, wl_resource_get_client(cua_dev[idx].entered->resource));
			if (o) { uint32_t ls = wlr_seat_client_next_serial(o); struct wl_resource *r2; wl_resource_for_each(r2, &o->pointers) { wl_pointer_send_leave(r2, ls, cua_dev[idx].entered->resource); cua_frame(r2); } }
		}
		uint32_t es = wlr_seat_client_next_serial(sc);
		wl_resource_for_each(res, &sc->pointers) { wl_pointer_send_enter(res, es, surface->resource, sx, sy); cua_frame(res); }
		cua_dev[idx].entered = surface;
	}
	uint32_t tm = cua_now_ms();
	wl_resource_for_each(res, &sc->pointers) { wl_pointer_send_motion(res, tm, sx, sy); cua_frame(res); }
}
static void cua_button(struct tinywl_server *server, int idx, uint32_t button, bool pressed) {
	struct tinywl_toplevel *t = (idx >= 0 && idx < CUA_MAXDEV) ? cua_win[idx] : NULL;
	if (!t) return;
	struct wlr_surface *surface = t->xdg_toplevel->base->surface;
	struct wlr_seat_client *sc = wlr_seat_client_for_wl_client(server->seat, wl_resource_get_client(surface->resource));
	if (!sc || wl_list_empty(&sc->pointers)) return;
	if (cua_dev[idx].cursor) { wlr_scene_buffer_set_dest_size(cua_dev[idx].cursor, pressed ? 42 : 34, pressed ? 54 : 44); wlr_scene_node_raise_to_top(&cua_dev[idx].cursor->node); }
	uint32_t tm = cua_now_ms(), bs = wlr_seat_client_next_serial(sc);
	struct wl_resource *res;
	wl_resource_for_each(res, &sc->pointers) {
		wl_pointer_send_button(res, bs, tm, button, pressed ? WL_POINTER_BUTTON_STATE_PRESSED : WL_POINTER_BUTTON_STATE_RELEASED);
		cua_frame(res);
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
	free(s); xkb_keymap_unref(km); xkb_context_unref(ctx);
	wlr_log(WLR_INFO, "[PHASE-E] xkb keymap ready (%zu bytes)", g_keymap_size);
}
static void cua_key(struct tinywl_server *server, int idx, uint32_t keycode, bool pressed) {
	struct tinywl_toplevel *t = (idx >= 0 && idx < CUA_MAXDEV) ? cua_win[idx] : NULL;
	if (!t) return;
	struct wlr_surface *surface = t->xdg_toplevel->base->surface;
	struct wlr_seat_client *sc = wlr_seat_client_for_wl_client(server->seat, wl_resource_get_client(surface->resource));
	if (!sc || wl_list_empty(&sc->keyboards)) return;
	struct wl_resource *res;
	if (cua_kbd[idx].entered != surface) {
		uint32_t es = wlr_seat_client_next_serial(sc);
		struct wl_array keys; wl_array_init(&keys);
		wl_resource_for_each(res, &sc->keyboards) {
			wl_keyboard_send_keymap(res, WL_KEYBOARD_KEYMAP_FORMAT_XKB_V1, g_keymap_fd, g_keymap_size);
			wl_keyboard_send_enter(res, es, surface->resource, &keys);
			wl_keyboard_send_modifiers(res, wlr_seat_client_next_serial(sc), 0, 0, 0, 0);
		}
		wl_array_release(&keys);
		cua_kbd[idx].entered = surface;
	}
	uint32_t tm = cua_now_ms(), ks = wlr_seat_client_next_serial(sc);
	wl_resource_for_each(res, &sc->keyboards) {
		wl_keyboard_send_key(res, ks, tm, keycode, pressed ? WL_KEYBOARD_KEY_STATE_PRESSED : WL_KEYBOARD_KEY_STATE_RELEASED);
	}
}
static int eis_loop_cb(int fd, uint32_t mask, void *data) {
	struct tinywl_server *server = data;
	eis_dispatch(g_eis);
	struct eis_event *e;
	while ((e = eis_get_event(g_eis)) != NULL) {
		switch (eis_event_get_type(e)) {
		case EIS_EVENT_CLIENT_CONNECT: {
			struct eis_client *client = eis_event_get_client(e);
			wlr_log(WLR_INFO, "[PHASE-E] EIS client connect; %d devices (%d keyboard)", g_ndev, g_nkbd);
			eis_client_connect(client);
			struct eis_seat *seat = eis_client_new_seat(client, "cua-seat");
			eis_seat_configure_capability(seat, EIS_DEVICE_CAP_POINTER_ABSOLUTE);
			eis_seat_configure_capability(seat, EIS_DEVICE_CAP_BUTTON);
			eis_seat_configure_capability(seat, EIS_DEVICE_CAP_KEYBOARD);
			eis_seat_add(seat);
			break;
		}
		case EIS_EVENT_SEAT_BIND: {
			struct eis_seat *seat = eis_event_get_seat(e);
			for (int i = 0; i < g_ndev && i < CUA_MAXDEV; i++) {
				struct eis_device *dev = eis_seat_new_device(seat);
				char nm[64];
				if (i < g_nkbd) {
					snprintf(nm, sizeof nm, "cua-keyboard-%d", i);
					eis_device_configure_name(dev, nm);
					eis_device_configure_type(dev, EIS_DEVICE_TYPE_VIRTUAL);
					eis_device_configure_capability(dev, EIS_DEVICE_CAP_KEYBOARD);
					struct eis_keymap *kmap = eis_device_new_keymap(dev, EIS_KEYMAP_TYPE_XKB, g_keymap_fd, g_keymap_size);
					eis_keymap_add(kmap);
				} else {
					snprintf(nm, sizeof nm, "cua-pointer-%d", i);
					eis_device_configure_name(dev, nm);
					eis_device_configure_type(dev, EIS_DEVICE_TYPE_VIRTUAL);
					eis_device_configure_capability(dev, EIS_DEVICE_CAP_POINTER_ABSOLUTE);
					eis_device_configure_capability(dev, EIS_DEVICE_CAP_BUTTON);
					struct eis_region *r = eis_device_new_region(dev);
					eis_region_set_offset(r, 0, 0);
					eis_region_set_size(r, 1920, 1080);
					eis_region_add(r);
				}
				eis_device_set_user_data(dev, (void *)(intptr_t)i);
				eis_device_add(dev);
				eis_device_resume(dev);
			}
			wlr_log(WLR_INFO, "[PHASE-E] SEAT_BIND -> created %d devices", g_ndev);
			break;
		}
		case EIS_EVENT_POINTER_MOTION_ABSOLUTE: {
			int idx = (int)(intptr_t)eis_device_get_user_data(eis_event_get_device(e));
			cua_motion(server, idx, eis_event_pointer_get_absolute_x(e), eis_event_pointer_get_absolute_y(e));
			break;
		}
		case EIS_EVENT_BUTTON_BUTTON: {
			int idx = (int)(intptr_t)eis_device_get_user_data(eis_event_get_device(e));
			cua_button(server, idx, eis_event_button_get_button(e), eis_event_button_get_is_press(e));
			break;
		}
		case EIS_EVENT_KEYBOARD_KEY: {
			int idx = (int)(intptr_t)eis_device_get_user_data(eis_event_get_device(e));
			cua_key(server, idx, eis_event_keyboard_get_key(e), eis_event_keyboard_get_key_is_press(e));
			break;
		}
		default: break;
		}
		eis_event_unref(e);
	}
	wl_display_flush_clients(server->wl_display);
	return 0;
}

"""

EIS_SETUP = """	wlr_screencopy_manager_v1_create(server.wl_display);
	{
		const char *nd = getenv("CUA_NDEV"); if (nd) g_ndev = atoi(nd);
		const char *nk = getenv("CUA_NKBD"); if (nk) g_nkbd = atoi(nk);
		cua_init_keymap();
		const char *eis_sock = getenv("CUA_EIS_SOCKET");
		if (eis_sock == NULL) eis_sock = "cua-eis-0";
		g_eis = eis_new(&server);
		if (g_eis == NULL || eis_setup_backend_socket(g_eis, eis_sock) != 0) {
			wlr_log(WLR_ERROR, "[PHASE-E] failed to set up EIS socket '%s'", eis_sock);
		} else {
			struct wl_event_loop *eloop = wl_display_get_event_loop(server.wl_display);
			wl_event_loop_add_fd(eloop, eis_get_fd(g_eis), WL_EVENT_READABLE, eis_loop_cb, &server);
			wlr_log(WLR_INFO, "[PHASE-E] EIS server on '%s', ndev=%d nkbd=%d", eis_sock, g_ndev, g_nkbd);
		}
	}
	"""

GRID = """wl_list_insert(&toplevel->server->toplevels, &toplevel->link);
	{ /* tile windows into a grid (and place a non-driven foreground window) */
		static int g_place = 0; int idx = g_place++;
		if (idx >= 0 && idx < CUA_MAXDEV) cua_win[idx] = toplevel;
		int cols = 4, cw = 470, ch = 270, fidx = -1, fx = 0, fy = 0; const char *e;
		if ((e = getenv("CUA_COLS"))) cols = atoi(e);
		if ((e = getenv("CUA_CELLW"))) cw = atoi(e);
		if ((e = getenv("CUA_CELLH"))) ch = atoi(e);
		if ((e = getenv("CUA_FRONT_IDX"))) fidx = atoi(e);
		if ((e = getenv("CUA_FRONT_X"))) fx = atoi(e);
		if ((e = getenv("CUA_FRONT_Y"))) fy = atoi(e);
		if (idx == fidx)
			wlr_scene_node_set_position(&toplevel->scene_tree->node, fx, fy);
		else
			wlr_scene_node_set_position(&toplevel->scene_tree->node, (idx % cols) * cw, (idx / cols) * ch);
	}"""

def repl(s, old, new, label):
    if s.count(old) != 1:
        print("ANCHOR FAIL [%s]: count=%d" % (label, s.count(old))); sys.exit(1)
    print("ok [%s]" % label); return s.replace(old, new)

src = repl(src, "struct tinywl_server {", INCLUDES + "struct tinywl_server {", "includes")
src = repl(src,
    'server.seat = wlr_seat_create(server.wl_display, "seat0");',
    'server.seat = wlr_seat_create(server.wl_display, "seat0");\n\twlr_seat_set_capabilities(server.seat, WL_SEAT_CAPABILITY_POINTER | WL_SEAT_CAPABILITY_KEYBOARD);',
    "force-caps")
src = repl(src, "int main(int argc, char *argv[]) {", FUNCS + "int main(int argc, char *argv[]) {", "funcs")
src = repl(src, "wl_display_run(server.wl_display);", EIS_SETUP + "wl_display_run(server.wl_display);", "eis-setup")
src = repl(src, "wl_list_insert(&toplevel->server->toplevels, &toplevel->link);", GRID, "grid-placement")
src = repl(src,
    "\tstruct wlr_output_mode *mode = wlr_output_preferred_mode(wlr_output);\n\tif (mode != NULL) {\n\t\twlr_output_state_set_mode(&state, mode);\n\t}",
    "\tstruct wlr_output_mode *mode = wlr_output_preferred_mode(wlr_output);\n\tif (mode != NULL) {\n\t\twlr_output_state_set_mode(&state, mode);\n\t} else {\n\t\tint ow = getenv(\"CUA_OUTW\") ? atoi(getenv(\"CUA_OUTW\")) : 1920;\n\t\tint oh = getenv(\"CUA_OUTH\") ? atoi(getenv(\"CUA_OUTH\")) : 1080;\n\t\twlr_output_state_set_custom_mode(&state, ow, oh, 0);\n\t}",
    "headless-custom-mode")
src = repl(src,
    "server.output_layout = wlr_output_layout_create(server.wl_display);",
    "server.output_layout = wlr_output_layout_create(server.wl_display);\n\twlr_xdg_output_manager_v1_create(server.wl_display, server.output_layout);",
    "xdg-output-manager")
io.open(P, "w", encoding="utf-8").write(src)
print("PHASE-G PATCHED OK")
