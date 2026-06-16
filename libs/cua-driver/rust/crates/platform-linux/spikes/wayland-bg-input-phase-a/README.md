# Phase A — libei/EIS "remote-control" path, end-to-end

Builds on [Spike 0](../wayland-bg-input/) (which proved focus-independent
`wl_pointer` injection is accepted by GTK4/Qt6/Chromium). Phase A wraps that
injection in the **libei/EIS** stack — the same protocol family Zoom/RDP use —
so cua-driver can stay a standard libei client while all the
background/per-window magic lives in a **cua-controlled compositor's EIS
server**. No upstream dependency; no `xdg-desktop-portal`; headless.

## Architecture proven here

```
 cua-driver (libei client)                 cua's compositor (tinywl + EIS server)
 ─────────────────────────                 ──────────────────────────────────────
 ei_new_sender()                           eis_new() + eis_setup_backend_socket()
 ei_setup_backend_socket(LIBEI_SOCKET) ───▶ EIS_EVENT_CLIENT_CONNECT → accept
 ei_seat_bind_capabilities(ABS, BUTTON) ──▶ SEAT_BIND → create virtual abs-pointer
 ei_device_pointer_motion_absolute() ─────▶ EIS_EVENT_POINTER_MOTION_ABSOLUTE
 ei_device_button_button(BTN_LEFT) ───────▶ EIS_EVENT_BUTTON_BUTTON
                                              └─▶ route to UNFOCUSED window:
                                                  wl_pointer_send_enter/motion/button
                                                  directly to its pointer resource
                                                  (no wlr_seat focus, no cursor move)
```

A **direct socket** (`$CUA_EIS_SOCKET` / `LIBEI_SOCKET`) is used — the portal +
permission dialog only gate untrusted clients on a real desktop; here we own
both ends, so it is skipped (headless-viable).

## Result (on the desktop-workspace-wayland VM, wlroots 0.19, headless)

```
CLIENT: connected to EIS socket '/tmp/spike-xdg/cua-eis-0'
CLIENT: bound seat caps (pointer-absolute + button)
CLIENT: device resumed (ready to emulate)
CLIENT: sent click #1..8 via libei
[PHASE-A] EIS client connect → SEAT_BIND → created + resumed virtual abs-pointer
[PHASE-A] EIS button 272 press=1 -> route
[PHASE-A] injected click to UNFOCUSED 'cua-spike-gtk4' at 114,87
GTK4: button CLICKED (activated)        ← ×8
```

A libei client drove a real click into an **unfocused** GTK4 window through the
EIS server, with no portal, no real input device, no global cursor, no focus
change, and no window raise.

## Files

| File | Purpose |
|---|---|
| `provision.sh` | Idempotent VM setup (the containerDisk is ephemeral — re-run after every reset). Installs build deps + `libei-dev`/`libeis-dev` + test clients. |
| `tinywl.eis.patch` | Unified diff vs wlroots **0.19** `tinywl/tinywl.c`: embeds an EIS server (`libeis`) on the wl event loop and routes emulated input to unfocused windows via the Spike-0 injection. |
| `phase_a_patch.py` | The generator that produces the above patch (idempotent, asserts anchors). |
| `ei_client.c` | Minimal libei client (stand-in for cua-driver's future libei backend): binds a virtual absolute pointer and emits clicks. |
| `gtk4_test.c` | GTK4 target that logs pointer enter/press + button activation. |

## Run

```bash
./provision.sh                      # once per VM boot (ephemeral)
cd /opt/spike                       # fetch wlroots 0.19 tinywl/tinywl.c here
python3 phase_a_patch.py            # patch ./tinywl.c in place
wayland-scanner server-header /usr/share/wayland-protocols/stable/xdg-shell/xdg-shell.xml xdg-shell-protocol.h
wayland-scanner private-code  /usr/share/wayland-protocols/stable/xdg-shell/xdg-shell.xml xdg-shell-protocol.c
gcc -DWLR_USE_UNSTABLE -I. tinywl.c xdg-shell-protocol.c -o tinywl \
    $(pkg-config --cflags --libs wlroots-0.19 wayland-server xkbcommon pixman-1 libeis-1.0)
gcc ei_client.c -o ei_client $(pkg-config --cflags --libs libei-1.0)
gcc gtk4_test.c -o gtk4_test $(pkg-config --cflags --libs gtk4)

export XDG_RUNTIME_DIR=/tmp/spike-xdg; mkdir -p $XDG_RUNTIME_DIR; chmod 700 $XDG_RUNTIME_DIR
export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_HEADLESS_OUTPUTS=1
export CUA_EIS_SOCKET=$XDG_RUNTIME_DIR/cua-eis-0
./tinywl -s "stdbuf -oL ./gtk4_test >/tmp/gtk.log 2>&1 & sleep 5; wev >/dev/null 2>&1 & sleep 3; ./ei_client >/tmp/ei.log 2>&1 &"
# /tmp/gtk.log shows "button CLICKED"; tinywl stderr shows the [PHASE-A] routing.
```

## Scope / caveats (this is a skeleton, not the product)

- **Routing is a placeholder**: every EIS button-press clicks *all* unfocused
  windows at their content-center; it ignores the client's absolute coordinates
  and has no device→window addressing. Real work: map EIS absolute coords (and
  per-device target) to a specific surface + surface-local point.
- **enter/leave bookkeeping** is not yet correct (re-enters each click — Qt
  warned about this in the spike). Track per-(device,surface) enter/leave.
- **Keyboard, motion, scroll, drags** not wired yet (only click).
- Compositor vehicle is still tinywl; the real target is a vendored labwc fork.

## Next (Phase B)

Per-window device→surface routing + coordinate mapping; enter/leave/motion/drag
correctness; keyboard; then port from tinywl to the vendored labwc compositor
and drive it from cua-driver's `platform-linux` via the `reis` crate.
