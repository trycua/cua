# Spike 0 — native-Wayland backgrounded per-window input injection

**Question:** On native Wayland (no XWayland), can we deliver pointer input to a
**specific, unfocused window** — without raising it, without moving the user's
real cursor, and without a real input device — such that **real UI toolkits act
on it**? This is the load-bearing unknown for adding background, multi-cursor
computer-use to cua-driver on Wayland (the "16-window cursive" demo), via the
libei/EIS "remote-control" route where the EIS server lives inside a
cua-controlled compositor.

**Answer: YES.** Validated for raw libwayland (`wev`), **GTK4**, **Qt6**, and
**Chromium** (Google Chrome 149 — the accessibility-less / Electron-class case).

## Mechanism

A Wayland client cannot inject into another client; only the compositor can.
The compositor delivers pointer events straight to the **target client's
existing `wl_pointer` resources**, bypassing the `wlr_seat` focus machinery
entirely (the Wayland analogue of X11 `XSendEvent`-to-XID):

```c
struct wl_client *c = wl_resource_get_client(surface->resource);
struct wlr_seat_client *sc = wlr_seat_client_for_wl_client(seat, c);
uint32_t serial = wlr_seat_client_next_serial(sc);
struct wl_resource *res;
wl_resource_for_each(res, &sc->pointers) {
    wl_pointer_send_enter(res, serial, surface->resource, sx, sy);
    wl_pointer_send_motion(res, time, sx, sy);
    wl_pointer_send_button(res, serial2, time, BTN_LEFT, WL_POINTER_BUTTON_STATE_PRESSED);
    /* ... released, frame ... */
}
```

`wlr_seat`'s focus/cursor state is never touched: no `wlr_seat_pointer_notify_*`,
no `server.seat.cursor` movement, no window raise.

## What's here

| File | Purpose |
|---|---|
| `tinywl.spike.patch` | Unified diff against wlroots **0.19** `tinywl/tinywl.c`. Adds focus-bypassing injection on a timer (no keyboard needed) into every non-focused toplevel. |
| `spike_patch.py` | The generator that produces the above (idempotent, asserts anchors). |
| `gtk4_test.c` | Minimal GTK4 client logging pointer enter/press + button **activation**. |
| `qt6_test.cpp` | Minimal Qt6 widget client logging enter/press + button **activation**. |
| `spike.html` | Chromium test page; button `onclick` sets `document.title` (observed via `xdg_toplevel.set_title`, so no rendering/visuals needed). |

The compositor vehicle is **tinywl** (wlroots' ~1000-line reference compositor),
not labwc, because the question is about wlroots seat mechanics + toolkit
behavior, which are compositor-agnostic. tinywl is trivial to patch and build.

## Results

Each toolkit ran as an **unfocused** target (a second client held focus) under
headless tinywl; injection fired at the window's content center:

| Client | Outcome on the unfocused window |
|---|---|
| `wev` (raw libwayland) | received `enter`/`motion`/`button`; the focused client received nothing |
| **GTK4** | `ENTER → MOTION → PRESSED → button CLICKED` |
| **Qt6** | `ENTER → PRESSED → button CLICKED` |
| **Chromium** | DOM `onclick` fired → title → `CLICKED-OK` (renderer hit-test + JS ran) |

## Implementation learnings (must carry into the real EIS server)

1. **Inject at the xdg window-geometry content center**, not the surface
   origin. Toolkits with client-side decorations (GTK4/Qt) exclude the
   shadow/resize margin from their input region, so naïve `(30,30)` is silently
   dropped. Use `xdg_toplevel->base->geometry` (x/y/w/h). Raw clients (`wev`)
   have no such margin, which is why they reacted to `(30,30)` and GTK didn't.
2. **Send `wl_pointer.leave` before re-`enter`** on the same surface. Qt warns
   that back-to-back enters violate the protocol (it works around it); the real
   server must track enter/leave state per (device, surface).
3. **Headless has no input devices**, so the compositor must explicitly
   `wlr_seat_set_capabilities(seat, WL_SEAT_CAPABILITY_POINTER)` or clients
   never bind a `wl_pointer`.

## Reproduce

On an Ubuntu 26.04 + wlroots-0.19 host (e.g. the `desktop-workspace-wayland`
VM), headless:

```bash
# deps
apt-get install -y meson ninja-build pkg-config libwayland-bin wayland-protocols \
    libwayland-dev libwlroots-0.19-dev libxkbcommon-dev libpixman-1-dev \
    wev libgtk-4-dev qt6-base-dev qt6-wayland
# patch + build tinywl (fetch wlroots 0.19 tinywl/tinywl.c first)
python3 spike_patch.py                       # edits ./tinywl.c in place
wayland-scanner server-header /usr/share/wayland-protocols/stable/xdg-shell/xdg-shell.xml xdg-shell-protocol.h
wayland-scanner private-code  /usr/share/wayland-protocols/stable/xdg-shell/xdg-shell.xml xdg-shell-protocol.c
gcc -DWLR_USE_UNSTABLE -I. tinywl.c xdg-shell-protocol.c -o tinywl \
    $(pkg-config --cflags --libs wlroots-0.19 wayland-server xkbcommon pixman-1)
# build test clients
gcc gtk4_test.c -o gtk4_test $(pkg-config --cflags --libs gtk4)
g++ -std=c++17 qt6_test.cpp -o qt6_test $(pkg-config --cflags --libs Qt6Widgets) -fPIC
# run (target maps first, wev steals focus second); inspect each client's log
export XDG_RUNTIME_DIR=/tmp/spike-xdg; mkdir -p $XDG_RUNTIME_DIR; chmod 700 $XDG_RUNTIME_DIR
export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_HEADLESS_OUTPUTS=1
./tinywl -s "stdbuf -oL ./gtk4_test >/tmp/gtk.log 2>&1 & sleep 6; wev >/dev/null 2>&1 &"
```

`tinywl` logs each injection (`[SPIKE] ...`); the client logs prove receipt.

## Next

This greenlights **Phase A** of the plan: stand up an EIS (libei) server inside
a cua-controlled compositor that performs this routing, driven by a libei client
over a direct socket. See the architecture note in the project plan.
