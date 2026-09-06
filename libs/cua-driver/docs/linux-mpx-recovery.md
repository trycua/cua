# Linux X11 MPX recovery

Refs #3337.

At main `130df4251e75f48bcbc43145b0cf7c7feac9ddc5`, a real Xorg 21.1.11
desktop with libinput and `/dev/uinput` delivers a normal background scroll
and removes its temporary master devices. SIGTERM during a longer scroll
leaves its master pointer/keyboard pair after the owning process exits.
The GTK keyboard fixture still accepts keys afterward in this environment;
this does not reproduce the separate Chrome keyboard symptom reported in
the issue.

Recovery must distinguish a stale owner from a live process, PID reuse,
inaccessible process metadata, and a different host or PID namespace using
the same X server. A bare PID parsed from an old device name is insufficient.
Unknown or legacy ownership must remain untouched by automatic recovery.

The candidate will be checked on real Xorg for normal input, clean exit,
SIGTERM and SIGKILL during an operation, restart cleanup, and live-peer
preservation. Focused ownership tests and the canonical Linux desktop
harness complement that native evidence. No Wayland behavior is changed.
