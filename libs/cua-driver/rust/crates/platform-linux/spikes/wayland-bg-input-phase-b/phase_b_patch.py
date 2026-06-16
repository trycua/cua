#!/usr/bin/env python3
# Phase B: multi-cursor, coordinate-accurate, per-window EIS routing in tinywl.
# N libei devices -> N distinct windows; each device's absolute coords map to its
# target window's surface-local space; proper enter/leave/motion bookkeeping.
import sys, io
P = "/opt/spike/tinywl.c"
src = io.open(P, encoding="utf-8").read()

INCLUDES = """#include <stdint.h>
#include <linux/input-event-codes.h>
#include <wayland-server-protocol.h>
#include <libeis.h>

#define CUA_MAXDEV 64
static struct eis *g_eis = NULL;
static int g_ndev = 16;
struct cua_devstate { struct wlr_surface *entered; };
static struct cua_devstate cua_dev[CUA_MAXDEV];

"""

FUNCS = r"""
static void cua_frame(struct wl_resource *res) {
	if (wl_resource_get_version(res) >= WL_POINTER_FRAME_SINCE_VERSION)
		wl_pointer_send_frame(res);
}
static struct tinywl_toplevel *cua_toplevel_by_index(struct tinywl_server *server, int idx) {
	int i = 0; struct tinywl_toplevel *t;
	wl_list_for_each(t, &server->toplevels, link) { if (i++ == idx) return t; }
	return NULL;
}
static void cua_send_leave(struct wlr_seat *seat, struct wlr_surface *surf) {
	if (!surf) return;
	struct wlr_seat_client *sc = wlr_seat_client_for_wl_client(seat, wl_resource_get_client(surf->resource));
	if (!sc) return;
	uint32_t serial = wlr_seat_client_next_serial(sc);
	struct wl_resource *res;
	wl_resource_for_each(res, &sc->pointers) { wl_pointer_send_leave(res, serial, surf->resource); cua_frame(res); }
}
/* Map device idx's content-relative (x,y) to its target window and inject motion,
 * sending enter (and a leave of any previously-entered surface) only on change. */
static void cua_motion(struct tinywl_server *server, int idx, double x, double y) {
	struct tinywl_toplevel *t = cua_toplevel_by_index(server, idx);
	if (!t) return;
	struct wlr_surface *surface = t->xdg_toplevel->base->surface;
	struct wlr_box geo = t->xdg_toplevel->base->geometry;
	wl_fixed_t sx = wl_fixed_from_double(geo.x + x), sy = wl_fixed_from_double(geo.y + y);
	struct wlr_seat_client *sc = wlr_seat_client_for_wl_client(server->seat, wl_resource_get_client(surface->resource));
	if (!sc || wl_list_empty(&sc->pointers)) return;
	struct wl_resource *res;
	if (cua_dev[idx].entered != surface) {
		if (cua_dev[idx].entered) cua_send_leave(server->seat, cua_dev[idx].entered);
		uint32_t es = wlr_seat_client_next_serial(sc);
		wl_resource_for_each(res, &sc->pointers) { wl_pointer_send_enter(res, es, surface->resource, sx, sy); cua_frame(res); }
		cua_dev[idx].entered = surface;
	}
	struct timespec ts; clock_gettime(CLOCK_MONOTONIC, &ts);
	uint32_t tm = (uint32_t)(ts.tv_sec * 1000 + ts.tv_nsec / 1000000);
	wl_resource_for_each(res, &sc->pointers) { wl_pointer_send_motion(res, tm, sx, sy); cua_frame(res); }
}
static void cua_button(struct tinywl_server *server, int idx, uint32_t button, bool pressed) {
	struct tinywl_toplevel *t = cua_toplevel_by_index(server, idx);
	if (!t) return;
	struct wlr_surface *surface = t->xdg_toplevel->base->surface;
	struct wlr_seat_client *sc = wlr_seat_client_for_wl_client(server->seat, wl_resource_get_client(surface->resource));
	if (!sc || wl_list_empty(&sc->pointers)) return;
	struct timespec ts; clock_gettime(CLOCK_MONOTONIC, &ts);
	uint32_t tm = (uint32_t)(ts.tv_sec * 1000 + ts.tv_nsec / 1000000);
	uint32_t bs = wlr_seat_client_next_serial(sc);
	struct wl_resource *res;
	wl_resource_for_each(res, &sc->pointers) {
		wl_pointer_send_button(res, bs, tm, button,
			pressed ? WL_POINTER_BUTTON_STATE_PRESSED : WL_POINTER_BUTTON_STATE_RELEASED);
		cua_frame(res);
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
			wlr_log(WLR_INFO, "[PHASE-B] EIS client connect; will expose %d devices", g_ndev);
			eis_client_connect(client);
			struct eis_seat *seat = eis_client_new_seat(client, "cua-seat");
			eis_seat_configure_capability(seat, EIS_DEVICE_CAP_POINTER_ABSOLUTE);
			eis_seat_configure_capability(seat, EIS_DEVICE_CAP_BUTTON);
			eis_seat_add(seat);
			break;
		}
		case EIS_EVENT_SEAT_BIND: {
			if (eis_event_seat_has_capability(e, EIS_DEVICE_CAP_POINTER_ABSOLUTE)) {
				struct eis_seat *seat = eis_event_get_seat(e);
				for (int i = 0; i < g_ndev && i < CUA_MAXDEV; i++) {
					struct eis_device *dev = eis_seat_new_device(seat);
					char nm[64]; snprintf(nm, sizeof nm, "cua-cursor-%d", i);
					eis_device_configure_name(dev, nm);
					eis_device_configure_type(dev, EIS_DEVICE_TYPE_VIRTUAL);
					eis_device_configure_capability(dev, EIS_DEVICE_CAP_POINTER_ABSOLUTE);
					eis_device_configure_capability(dev, EIS_DEVICE_CAP_BUTTON);
					struct eis_region *r = eis_device_new_region(dev);
					eis_region_set_offset(r, 0, 0);
					eis_region_set_size(r, 1920, 1080);
					eis_region_add(r);
					eis_device_set_user_data(dev, (void *)(intptr_t)i);
					eis_device_add(dev);
					eis_device_resume(dev);
				}
				wlr_log(WLR_INFO, "[PHASE-B] SEAT_BIND -> created + resumed %d virtual abs-pointers", g_ndev);
			}
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
		default: break;
		}
		eis_event_unref(e);
	}
	wl_display_flush_clients(server->wl_display);
	return 0;
}

"""

EIS_SETUP = """	{
		const char *nd = getenv("CUA_NDEV"); if (nd) g_ndev = atoi(nd);
		const char *eis_sock = getenv("CUA_EIS_SOCKET");
		if (eis_sock == NULL) eis_sock = "cua-eis-0";
		g_eis = eis_new(&server);
		if (g_eis == NULL || eis_setup_backend_socket(g_eis, eis_sock) != 0) {
			wlr_log(WLR_ERROR, "[PHASE-B] failed to set up EIS socket '%s'", eis_sock);
		} else {
			struct wl_event_loop *eloop = wl_display_get_event_loop(server.wl_display);
			wl_event_loop_add_fd(eloop, eis_get_fd(g_eis), WL_EVENT_READABLE, eis_loop_cb, &server);
			wlr_log(WLR_INFO, "[PHASE-B] EIS server on '%s', ndev=%d", eis_sock, g_ndev);
		}
	}
	"""

def repl(s, old, new, label):
    if s.count(old) != 1:
        print("ANCHOR FAIL [%s]: count=%d" % (label, s.count(old))); sys.exit(1)
    print("ok [%s]" % label); return s.replace(old, new)

src = repl(src, "struct tinywl_server {", INCLUDES + "struct tinywl_server {", "includes")
src = repl(src,
    'server.seat = wlr_seat_create(server.wl_display, "seat0");',
    'server.seat = wlr_seat_create(server.wl_display, "seat0");\n\twlr_seat_set_capabilities(server.seat, WL_SEAT_CAPABILITY_POINTER);',
    "force-pointer-cap")
src = repl(src, "int main(int argc, char *argv[]) {", FUNCS + "int main(int argc, char *argv[]) {", "funcs")
src = repl(src, "wl_display_run(server.wl_display);", EIS_SETUP + "wl_display_run(server.wl_display);", "eis-setup")
io.open(P, "w", encoding="utf-8").write(src)
print("PHASE-B PATCHED OK")
