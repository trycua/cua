#!/usr/bin/env python3
# Phase A: turn tinywl (wlroots 0.19) into an EIS-server compositor.
# A libei client connects over a direct socket; the embedded EIS server routes
# its emulated pointer/button events to UNFOCUSED windows via direct wl_pointer
# injection (the spike mechanism), with no wlr_seat focus change.
import sys, io
P = "/opt/spike/tinywl.c"
src = io.open(P, encoding="utf-8").read()

INCLUDES = """#include <linux/input-event-codes.h>
#include <wayland-server-protocol.h>
#include <libeis.h>

static struct eis *g_eis = NULL;

"""

FUNCS = r"""
/* ---- injection core (validated in Spike 0): deliver wl_pointer events
 * directly to a client's pointer resources, bypassing wlr_seat focus. ---- */
static uint32_t cua_now_ms(void) {
	struct timespec ts; clock_gettime(CLOCK_MONOTONIC, &ts);
	return (uint32_t)(ts.tv_sec * 1000 + ts.tv_nsec / 1000000);
}
static void cua_frame(struct wl_resource *res) {
	if (wl_resource_get_version(res) >= WL_POINTER_FRAME_SINCE_VERSION)
		wl_pointer_send_frame(res);
}
static void cua_inject_click(struct tinywl_server *server, struct tinywl_toplevel *t) {
	struct wlr_surface *surface = t->xdg_toplevel->base->surface;
	const char *title = t->xdg_toplevel->title ? t->xdg_toplevel->title : "(none)";
	struct wl_client *client = wl_resource_get_client(surface->resource);
	struct wlr_seat_client *sc = wlr_seat_client_for_wl_client(server->seat, client);
	if (!sc) { wlr_log(WLR_ERROR, "[PHASE-A] '%s': client never bound seat", title); return; }
	if (wl_list_empty(&sc->pointers)) { wlr_log(WLR_ERROR, "[PHASE-A] '%s': no wl_pointer", title); return; }
	/* content-center via xdg geometry, to clear CSD margins (Spike 0 learning) */
	struct wlr_box geo = t->xdg_toplevel->base->geometry;
	int cx = geo.width > 0 ? geo.x + geo.width / 2 : 100;
	int cy = geo.height > 0 ? geo.y + geo.height / 2 : 75;
	wl_fixed_t sx = wl_fixed_from_int(cx), sy = wl_fixed_from_int(cy);
	uint32_t tm = cua_now_ms();
	struct wl_resource *res;
	uint32_t es = wlr_seat_client_next_serial(sc);
	wl_resource_for_each(res, &sc->pointers) {
		wl_pointer_send_enter(res, es, surface->resource, sx, sy);
		wl_pointer_send_motion(res, tm, sx, sy); cua_frame(res);
	}
	uint32_t bs = wlr_seat_client_next_serial(sc);
	wl_resource_for_each(res, &sc->pointers) {
		wl_pointer_send_button(res, bs, tm + 5, BTN_LEFT, WL_POINTER_BUTTON_STATE_PRESSED); cua_frame(res);
	}
	uint32_t rs = wlr_seat_client_next_serial(sc);
	wl_resource_for_each(res, &sc->pointers) {
		wl_pointer_send_button(res, rs, tm + 15, BTN_LEFT, WL_POINTER_BUTTON_STATE_RELEASED); cua_frame(res);
	}
	wlr_log(WLR_INFO, "[PHASE-A] injected click to UNFOCUSED '%s' at %d,%d", title, cx, cy);
}
static void cua_route_click(struct tinywl_server *server) {
	if (wl_list_length(&server->toplevels) < 1) return;
	struct wlr_surface *kfocus = server->seat->keyboard_state.focused_surface;
	struct tinywl_toplevel *t;
	wl_list_for_each(t, &server->toplevels, link) {
		if (t->xdg_toplevel->base->surface == kfocus) continue; /* skip focused */
		cua_inject_click(server, t);
	}
	wl_display_flush_clients(server->wl_display);
}

/* ---- EIS server: accept a libei client, expose a virtual abs-pointer, and
 * route its emulated input through cua_route_click(). ---- */
static int eis_loop_cb(int fd, uint32_t mask, void *data) {
	struct tinywl_server *server = data;
	eis_dispatch(g_eis);
	struct eis_event *e;
	while ((e = eis_get_event(g_eis)) != NULL) {
		switch (eis_event_get_type(e)) {
		case EIS_EVENT_CLIENT_CONNECT: {
			struct eis_client *client = eis_event_get_client(e);
			wlr_log(WLR_INFO, "[PHASE-A] EIS client connect: %s", eis_client_get_name(client));
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
				struct eis_device *dev = eis_seat_new_device(seat);
				eis_device_configure_name(dev, "cua-virtual-abs-pointer");
				eis_device_configure_type(dev, EIS_DEVICE_TYPE_VIRTUAL);
				eis_device_configure_capability(dev, EIS_DEVICE_CAP_POINTER_ABSOLUTE);
				eis_device_configure_capability(dev, EIS_DEVICE_CAP_BUTTON);
				struct eis_region *r = eis_device_new_region(dev);
				eis_region_set_offset(r, 0, 0);
				eis_region_set_size(r, 1920, 1080);
				eis_region_add(r);
				eis_device_add(dev);
				eis_device_resume(dev);
				wlr_log(WLR_INFO, "[PHASE-A] SEAT_BIND -> created + resumed virtual abs-pointer");
			}
			break;
		}
		case EIS_EVENT_POINTER_MOTION_ABSOLUTE:
			wlr_log(WLR_INFO, "[PHASE-A] EIS motion_absolute %.1f,%.1f",
				eis_event_pointer_get_absolute_x(e), eis_event_pointer_get_absolute_y(e));
			break;
		case EIS_EVENT_BUTTON_BUTTON:
			wlr_log(WLR_INFO, "[PHASE-A] EIS button %u press=%d -> route",
				eis_event_button_get_button(e), eis_event_button_get_is_press(e));
			if (eis_event_button_get_is_press(e))
				cua_route_click(server);
			break;
		default: break;
		}
		eis_event_unref(e);
	}
	return 0;
}

"""

EIS_SETUP = """	{
		const char *eis_sock = getenv("CUA_EIS_SOCKET");
		if (eis_sock == NULL) eis_sock = "cua-eis-0";
		g_eis = eis_new(&server);
		if (g_eis == NULL || eis_setup_backend_socket(g_eis, eis_sock) != 0) {
			wlr_log(WLR_ERROR, "[PHASE-A] failed to set up EIS socket '%s'", eis_sock);
		} else {
			struct wl_event_loop *eloop = wl_display_get_event_loop(server.wl_display);
			wl_event_loop_add_fd(eloop, eis_get_fd(g_eis), WL_EVENT_READABLE, eis_loop_cb, &server);
			wlr_log(WLR_INFO, "[PHASE-A] EIS server listening on '%s' (under XDG_RUNTIME_DIR)", eis_sock);
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
print("PHASE-A PATCHED OK")
