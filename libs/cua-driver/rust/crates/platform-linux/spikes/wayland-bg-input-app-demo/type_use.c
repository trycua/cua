/* Drives a full session over libei: types `make` then `./mycalc` into a
 * terminal (keyboard device -> terminal window), then clicks 7 * 6 = on the
 * calculator that appears (pointer device -> calc window). All background
 * injection through the EIS server; no focus, no real input devices. */
#include <libei.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <math.h>
#include <linux/input-event-codes.h>

#define MAXB 64
static char lbl[MAXB][8]; static double bx[MAXB], by[MAXB]; static int nb = 0;
static int find(const char *l) { for (int i = 0; i < nb; i++) if (!strcmp(lbl[i], l)) return i; return -1; }
static void load(void) {
	FILE *f = fopen("/tmp/calc_coords.txt", "r"); if (!f) return; char L[8]; double x, y;
	while (nb < MAXB && fscanf(f, "%7s %lf %lf", L, &x, &y) == 3) { strncpy(lbl[nb], L, 7); lbl[nb][7]=0; bx[nb]=x; by[nb]=y; nb++; } fclose(f);
}
static int ck(char c) {
	static const int K[26] = {KEY_A,KEY_B,KEY_C,KEY_D,KEY_E,KEY_F,KEY_G,KEY_H,KEY_I,KEY_J,KEY_K,KEY_L,KEY_M,KEY_N,KEY_O,KEY_P,KEY_Q,KEY_R,KEY_S,KEY_T,KEY_U,KEY_V,KEY_W,KEY_X,KEY_Y,KEY_Z};
	if (c >= 'a' && c <= 'z') return K[c-'a'];
	switch (c) { case ' ': return KEY_SPACE; case '.': return KEY_DOT; case '/': return KEY_SLASH;
		case '-': return KEY_MINUS; case '\n': return KEY_ENTER; }
	return -1;
}
static void type(struct ei *ei, struct ei_device *k, const char *s) {
	ei_device_start_emulating(k, 1);
	for (const char *p = s; *p; p++) {
		int kc = ck(*p); if (kc < 0) continue;
		ei_device_keyboard_key(k, kc, true);  ei_device_frame(k, ei_now(ei));
		ei_device_keyboard_key(k, kc, false); ei_device_frame(k, ei_now(ei));
		ei_dispatch(ei); usleep(75000);
	}
	ei_device_stop_emulating(k); ei_dispatch(ei);
	printf("TYPE: \"%s\"\n", s); fflush(stdout);
}
static double curx = 40, cury = 40;
static double ease(double t) { return t < 0.5 ? 2*t*t : 1 - pow(-2*t + 2, 2)/2; } /* ease-in-out */
static void glide(struct ei *ei, struct ei_device *d, double tx, double ty) {
	double sx = curx, sy = cury, dx = tx - sx, dy = ty - sy, dist = hypot(dx, dy);
	double nx = -dy / (dist + 1e-6), ny = dx / (dist + 1e-6), defl = dist * 0.16; /* gentle curve */
	double c1x = sx + dx*0.33 + nx*defl, c1y = sy + dy*0.33 + ny*defl;
	double c2x = sx + dx*0.66 + nx*defl, c2y = sy + dy*0.66 + ny*defl;
	int N = 30;
	for (int i = 1; i <= N; i++) {
		double u = ease((double)i / N), m = 1 - u;
		double x = m*m*m*sx + 3*m*m*u*c1x + 3*m*u*u*c2x + u*u*u*tx;
		double y = m*m*m*sy + 3*m*m*u*c1y + 3*m*u*u*c2y + u*u*u*ty;
		ei_device_pointer_motion_absolute(d, x, y); ei_device_frame(d, ei_now(ei));
		ei_dispatch(ei); usleep(14000);
	}
	curx = tx; cury = ty;
}
static void click(struct ei *ei, struct ei_device *d, const char *label) {
	int i = find(label); if (i < 0) { printf("USE: no '%s'\n", label); return; }
	ei_device_start_emulating(d, 1);
	glide(ei, d, bx[i], by[i]); usleep(130000);
	ei_device_button_button(d, BTN_LEFT, true);  ei_device_frame(d, ei_now(ei)); usleep(150000);
	ei_device_button_button(d, BTN_LEFT, false); ei_device_frame(d, ei_now(ei));
	ei_device_stop_emulating(d); ei_dispatch(ei);
	printf("CLICK: '%s' @ %.0f,%.0f\n", label, bx[i], by[i]); fflush(stdout); usleep(500000);
}
int main(void) {
	const char *sock = getenv("CUA_EIS_SOCKET"); if (!sock) sock = "cua-eis-0";
	struct ei *ei = ei_new_sender(NULL);
	if (!ei || ei_setup_backend_socket(ei, sock) != 0) { fprintf(stderr, "connect failed\n"); return 1; }
	struct ei_device *kbd = NULL, *ptr = NULL;
	struct pollfd pfd = { .fd = ei_get_fd(ei), .events = POLLIN };
	for (int w = 0; (!kbd || !ptr) && w < 8000; w += 200) {
		poll(&pfd, 1, 200); ei_dispatch(ei);
		struct ei_event *e;
		while ((e = ei_get_event(ei))) {
			enum ei_event_type t = ei_event_get_type(e);
			if (t == EI_EVENT_SEAT_ADDED)
				ei_seat_bind_capabilities(ei_event_get_seat(e), EI_DEVICE_CAP_POINTER_ABSOLUTE, EI_DEVICE_CAP_BUTTON, EI_DEVICE_CAP_KEYBOARD, NULL);
			else if (t == EI_EVENT_DEVICE_RESUMED) {
				struct ei_device *d = ei_event_get_device(e);
				if (ei_device_has_capability(d, EI_DEVICE_CAP_KEYBOARD)) kbd = d;
				else if (ei_device_has_capability(d, EI_DEVICE_CAP_POINTER_ABSOLUTE)) ptr = d;
			}
			ei_event_unref(e);
		}
	}
	if (!kbd || !ptr) { fprintf(stderr, "missing device (kbd=%p ptr=%p)\n", (void*)kbd, (void*)ptr); return 1; }
	printf("READY: keyboard + pointer devices live\n"); fflush(stdout);
	sleep(2);
	type(ei, kbd, "make\n");
	sleep(7);                         /* compile */
	type(ei, kbd, "./mycalc\n");
	sleep(3);                         /* calc window maps + exports coords */
	load();
	printf("USE: %d button coords; computing 7 * 6\n", nb); fflush(stdout);
	const char *seq[] = { "7", "*", "6", "=" };
	for (int i = 0; i < 4; i++) click(ei, ptr, seq[i]);
	printf("DONE\n"); fflush(stdout);
	for (;;) { poll(&pfd, 1, 500); ei_dispatch(ei); struct ei_event *e; while ((e = ei_get_event(ei))) ei_event_unref(e); }
	return 0;
}
