/* Minimal libei client: connect to an EIS server over a direct socket, bind a
 * virtual absolute pointer, and send periodic clicks. Stands in for cua-driver's
 * future libei input backend. */
#include <libei.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <stdbool.h>
#include <linux/input-event-codes.h>

int main(int argc, char **argv) {
	const char *sock = getenv("CUA_EIS_SOCKET");
	if (!sock) sock = "cua-eis-0";
	struct ei *ei = ei_new_sender(NULL);
	if (!ei || ei_setup_backend_socket(ei, sock) != 0) {
		fprintf(stderr, "CLIENT: failed to connect to EIS socket '%s'\n", sock);
		return 1;
	}
	printf("CLIENT: connected to EIS socket '%s'\n", sock); fflush(stdout);

	struct ei_device *dev = NULL;
	struct pollfd pfd = { .fd = ei_get_fd(ei), .events = POLLIN };
	int sent = 0;
	for (;;) {
		poll(&pfd, 1, 2000);          /* wake on data or every 2s */
		ei_dispatch(ei);
		struct ei_event *e;
		while ((e = ei_get_event(ei)) != NULL) {
			switch (ei_event_get_type(e)) {
			case EI_EVENT_SEAT_ADDED: {
				struct ei_seat *seat = ei_event_get_seat(e);
				ei_seat_bind_capabilities(seat,
					EI_DEVICE_CAP_POINTER_ABSOLUTE, EI_DEVICE_CAP_BUTTON, NULL);
				printf("CLIENT: bound seat caps (pointer-absolute + button)\n"); fflush(stdout);
				break;
			}
			case EI_EVENT_DEVICE_ADDED:
				dev = ei_event_get_device(e);
				printf("CLIENT: device added\n"); fflush(stdout);
				break;
			case EI_EVENT_DEVICE_RESUMED:
				dev = ei_event_get_device(e);
				printf("CLIENT: device resumed (ready to emulate)\n"); fflush(stdout);
				break;
			default: break;
			}
			ei_event_unref(e);
		}
		/* Once the device is live, emit a click every wakeup. */
		if (dev) {
			ei_device_start_emulating(dev, ++sent);
			ei_device_pointer_motion_absolute(dev, 300, 300);
			ei_device_frame(dev, ei_now(ei));
			ei_device_button_button(dev, BTN_LEFT, true);
			ei_device_frame(dev, ei_now(ei));
			ei_device_button_button(dev, BTN_LEFT, false);
			ei_device_frame(dev, ei_now(ei));
			ei_device_stop_emulating(dev);
			ei_dispatch(ei);
			printf("CLIENT: sent click #%d via libei\n", sent); fflush(stdout);
		}
	}
	return 0;
}
