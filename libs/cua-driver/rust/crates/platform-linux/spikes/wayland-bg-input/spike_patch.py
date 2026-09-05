#!/usr/bin/env python3
# Patch tinywl.c (wlroots 0.19) for Spike 0: focus-bypassing background pointer injection.
import sys, io

P = "/opt/spike/tinywl.c"
src = io.open(P, encoding="utf-8").read()

INCLUDES = """#include <linux/input-event-codes.h>
#include <wayland-server-protocol.h>

"""

FUNCS = r"""
/* SPIKE 0: focus-bypassing background pointer injection. Deliver wl_pointer
 * enter+motion+button DIRECTLY to a chosen UNFOCUSED toplevel's client pointer
 * resources, with NO wlr_seat focus change and NO real pointer device. Tests
 * whether toolkits act on input to a surface that does not hold seat focus. */
static uint32_t spike_now_ms(void) {
	struct timespec ts;
	clock_gettime(CLOCK_MONOTONIC, &ts);
	return (uint32_t)(ts.tv_sec * 1000 + ts.tv_nsec / 1000000);
}

static void spike_frame(struct wl_resource *res) {
	if (wl_resource_get_version(res) >= WL_POINTER_FRAME_SINCE_VERSION) {
		wl_pointer_send_frame(res);
	}
}

static void spike_inject_one(struct tinywl_server *server,
		struct tinywl_toplevel *t) {
	struct wlr_surface *surface = t->xdg_toplevel->base->surface;
	const char *title = t->xdg_toplevel->title ? t->xdg_toplevel->title : "(none)";
	struct wl_client *client = wl_resource_get_client(surface->resource);
	struct wlr_seat_client *sc =
		wlr_seat_client_for_wl_client(server->seat, client);
	if (sc == NULL) {
		wlr_log(WLR_ERROR, "[SPIKE] '%s': client never bound the seat", title);
		return;
	}
	int n = wl_list_length(&sc->pointers);
	if (n == 0) {
		wlr_log(WLR_ERROR, "[SPIKE] '%s': bound no wl_pointer", title);
		return;
	}
	/* Aim at the CONTENT center using xdg window geometry, so we clear any
	 * client-side-decoration shadow/margin that the client excludes from its
	 * input region (raw clients like wev have no such margin; GTK/Qt do). */
	struct wlr_box geo = t->xdg_toplevel->base->geometry;
	int cx = geo.width > 0 ? geo.x + geo.width / 2 : 100;
	int cy = geo.height > 0 ? geo.y + geo.height / 2 : 75;
	wlr_log(WLR_INFO, "[SPIKE] '%s' geometry %dx%d@%d,%d -> inject at %d,%d",
		title, geo.width, geo.height, geo.x, geo.y, cx, cy);
	wl_fixed_t sx = wl_fixed_from_int(cx), sy = wl_fixed_from_int(cy);
	uint32_t tm = spike_now_ms();
	struct wl_resource *res;
	uint32_t es = wlr_seat_client_next_serial(sc);
	wl_resource_for_each(res, &sc->pointers) {
		wl_pointer_send_enter(res, es, surface->resource, sx, sy);
		wl_pointer_send_motion(res, tm, sx, sy);
		spike_frame(res);
	}
	uint32_t bs = wlr_seat_client_next_serial(sc);
	wl_resource_for_each(res, &sc->pointers) {
		wl_pointer_send_button(res, bs, tm + 5, BTN_LEFT,
			WL_POINTER_BUTTON_STATE_PRESSED);
		spike_frame(res);
	}
	uint32_t rs = wlr_seat_client_next_serial(sc);
	wl_resource_for_each(res, &sc->pointers) {
		wl_pointer_send_button(res, rs, tm + 15, BTN_LEFT,
			WL_POINTER_BUTTON_STATE_RELEASED);
		spike_frame(res);
	}
	wlr_log(WLR_INFO, "[SPIKE] injected click to UNFOCUSED '%s' (%d ptr, e=%u b=%u r=%u)",
		title, n, es, bs, rs);
}

static void spike_inject(struct tinywl_server *server) {
	if (wl_list_length(&server->toplevels) < 2) {
		wlr_log(WLR_INFO, "[SPIKE] waiting: need >=2 toplevels, have %d",
			wl_list_length(&server->toplevels));
		return;
	}
	struct wlr_surface *kfocus = server->seat->keyboard_state.focused_surface;
	/* Inject into EVERY toplevel that does NOT hold keyboard focus. */
	struct tinywl_toplevel *t;
	wl_list_for_each(t, &server->toplevels, link) {
		struct wlr_surface *surface = t->xdg_toplevel->base->surface;
		const char *title = t->xdg_toplevel->title ? t->xdg_toplevel->title : "(none)";
		if (surface == kfocus) {
			wlr_log(WLR_INFO, "[SPIKE] skipping FOCUSED toplevel '%s'", title);
			continue;
		}
		spike_inject_one(server, t);
	}
	wl_display_flush_clients(server->wl_display);
}

static int spike_timer_cb(void *data) {
	struct tinywl_server *server = data;
	static int n = 0;
	n++;
	wlr_log(WLR_INFO, "[SPIKE] timer fire #%d", n);
	spike_inject(server);
	if (n < 10) {
		struct wl_event_loop *loop =
			wl_display_get_event_loop(server->wl_display);
		struct wl_event_source *src =
			wl_event_loop_add_timer(loop, spike_timer_cb, server);
		wl_event_source_timer_update(src, 2000);
	}
	return 0;
}

"""

TIMER = """	struct wl_event_loop *spike_loop = wl_display_get_event_loop(server.wl_display);
	struct wl_event_source *spike_src = wl_event_loop_add_timer(spike_loop, spike_timer_cb, &server);
	wl_event_source_timer_update(spike_src, 3000);
	wlr_log(WLR_INFO, "[SPIKE] armed injection timer (first fire +3s)");
	"""

def repl(s, old, new, label):
    c = s.count(old)
    if c != 1:
        print("ANCHOR FAIL [%s]: count=%d" % (label, c)); sys.exit(1)
    print("ok [%s]" % label)
    return s.replace(old, new)

src = repl(src, "struct tinywl_server {", INCLUDES + "struct tinywl_server {", "includes")
src = repl(src,
    'server.seat = wlr_seat_create(server.wl_display, "seat0");',
    'server.seat = wlr_seat_create(server.wl_display, "seat0");\n\twlr_seat_set_capabilities(server.seat, WL_SEAT_CAPABILITY_POINTER);',
    "force-pointer-cap")
src = repl(src, "int main(int argc, char *argv[]) {", FUNCS + "int main(int argc, char *argv[]) {", "inject-funcs")
src = repl(src, "wl_display_run(server.wl_display);", TIMER + "wl_display_run(server.wl_display);", "arm-timer")

io.open(P, "w", encoding="utf-8").write(src)
print("PATCHED OK")
