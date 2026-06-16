/* Phase B cursive multi-cursor libei client: bind N virtual absolute pointers
 * and drive each along its own cursive stroke, concurrently. Stand-in for
 * cua-driver's future libei backend (one device == one background "session"). */
#include <libei.h>
#include <math.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <stdbool.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#include <linux/input-event-codes.h>

#ifndef M_PI
#define M_PI 3.14159265358979323846
#endif
#define MAXDEV 64
#define STEPS 120

static struct ei_device *devs[MAXDEV];
static int ndev = 0;

static bool have(struct ei_device *d){ for(int i=0;i<ndev;i++) if(devs[i]==d) return true; return false; }

static void cursive(int d, int t, double *x, double *y){
	double u = (double)t/STEPS;
	*x = 24.0 + 264.0*u;
	*y = 100.0 + 46.0*sin(2.0*M_PI*2.0*u + d*0.55) * (0.35 + 0.65*sin(M_PI*u));
}

int main(int argc, char **argv){
	int want = 16; const char *n = getenv("CUA_NDEV"); if(n) want = atoi(n);
	const char *sock = getenv("CUA_EIS_SOCKET"); if(!sock) sock = "cua-eis-0";
	struct ei *ei = ei_new_sender(NULL);
	if(!ei || ei_setup_backend_socket(ei, sock)!=0){ fprintf(stderr,"CLIENT: connect failed\n"); return 1; }
	printf("CLIENT: connected, want %d devices\n", want); fflush(stdout);

	struct pollfd pfd = { .fd = ei_get_fd(ei), .events = POLLIN };
	/* Phase 1: collect devices (up to ~8s). */
	for(int waited=0; ndev<want && waited<8000; ){
		poll(&pfd,1,200); waited+=200; ei_dispatch(ei);
		struct ei_event *e;
		while((e=ei_get_event(ei))){
			enum ei_event_type t = ei_event_get_type(e);
			if(t==EI_EVENT_SEAT_ADDED){
				ei_seat_bind_capabilities(ei_event_get_seat(e),
					EI_DEVICE_CAP_POINTER_ABSOLUTE, EI_DEVICE_CAP_BUTTON, NULL);
			} else if(t==EI_EVENT_DEVICE_RESUMED){
				struct ei_device *d = ei_event_get_device(e);
				if(!have(d) && ndev<MAXDEV) devs[ndev++]=d;
			}
			ei_event_unref(e);
		}
	}
	printf("CLIENT: %d devices resumed; settling\n", ndev); fflush(stdout);
	sleep(1); ei_dispatch(ei);

	/* Phase 2: draw one cursive stroke per device, concurrently. */
	for(int i=0;i<ndev;i++) ei_device_start_emulating(devs[i], 1);
	double x,y;
	for(int i=0;i<ndev;i++){ cursive(i,0,&x,&y); ei_device_pointer_motion_absolute(devs[i],x,y); ei_device_frame(devs[i],ei_now(ei)); }
	for(int i=0;i<ndev;i++){ ei_device_button_button(devs[i],BTN_LEFT,true); ei_device_frame(devs[i],ei_now(ei)); }
	ei_dispatch(ei);
	for(int t=1;t<=STEPS;t++){
		for(int i=0;i<ndev;i++){ cursive(i,t,&x,&y); ei_device_pointer_motion_absolute(devs[i],x,y); ei_device_frame(devs[i],ei_now(ei)); }
		ei_dispatch(ei);
		usleep(15000);
	}
	for(int i=0;i<ndev;i++){ ei_device_button_button(devs[i],BTN_LEFT,false); ei_device_frame(devs[i],ei_now(ei)); }
	for(int i=0;i<ndev;i++) ei_device_stop_emulating(devs[i]);
	ei_dispatch(ei);
	printf("CLIENT: drew %d concurrent cursive strokes\n", ndev); fflush(stdout);

	/* keep connection alive so targets flush, until killed */
	for(;;){ poll(&pfd,1,500); ei_dispatch(ei); struct ei_event *e; while((e=ei_get_event(ei))) ei_event_unref(e); }
	return 0;
}
