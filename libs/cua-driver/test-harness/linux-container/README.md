# Linux dispatch-ladder lane — XFCE in a container

A reproducible rig for validating the cua-driver Linux **`delivery_mode`**
background/foreground ladder and the AT-SPI modalities against a **real window
manager** (xfwm4 + EWMH) and a **live AT-SPI tree** — the two things the
existing Xvfb harness (no WM) can't exercise.

The target is the `trycua/cua-xfce` image: a full XFCE desktop on X11 with an
accessibility bus, reachable over VNC and the computer-server API. It's a
richer target than Xvfb for exactly the rungs that need a WM (foreground / EWMH
activation) or a11y (AT-SPI element + at-point clicks).

## What it validates

`calc.sh` drives `galculator` through the four parity modalities; map each to a
`delivery_mode` rung and confirm the reported `path`:

| Modality | `delivery_mode` | Expected `path` | Driver-verifiable? |
|---|---|---|---|
| Element click (`element_index`) | `background` | `x11_atspi` (AT-SPI `do_action`) | yes (a11y action) |
| Pixel / vision click | `background` | `x11_atspi` (`do_action`-at-point) for AX apps; else MPX `x11_pixel` | yes when AT-SPI-at-point lands |
| Pixel click (escalated) | `foreground` | `x11_pixel_fg` (EWMH activate → inject → restore) | no — confirm via screenshot |
| `type_text` into editable | `background` | `ax` (AT-SPI `insertText`) | yes (`verified:true`) |
| `type_text`, non-editable focus | `foreground` | `key_events_fg` | no — confirm via screenshot |

Background pixel/vision click **lands** on X11: an AX app takes the focus-free
AT-SPI `do_action`-at-point path (`x11_atspi`) — the same background-pixel
behavior macOS/Windows have. It falls to the MPX virtual-pointer path
(`x11_pixel`, which needs a **real Xorg + `/dev/uinput`**, absent under
Xvnc/minimal containers) only for non-AX surfaces; escalate to `foreground`
there.

## Setup recipe

1. **Run the image** (Azure Container Instances is the recommended single-
   container host; the image is public on Docker Hub):

   ```
   az container create -g <rg> -n cua-xfce --image trycua/cua-xfce \
     --ports 8000 5901 --ip-address Public --os-type Linux --cpu 2 --memory 4
   ```

2. **Install cua-driver as the desktop user** (`install-local` refuses to run
   as root; the container exec lands as root, so use `runuser`). Sync the repo
   in or build from a checkout, then:

   ```
   chown -R cua /opt/cua /home/cua/.cargo /home/cua/.rustup
   runuser -u cua -- bash /opt/cua/libs/cua-driver/install-local.sh
   ```

3. **Drive the scenario** (the daemon now self-discovers the session bus, so no
   manual `DBUS_SESSION_BUS_ADDRESS` export is needed for `serve` — `calc.sh`
   still exports it for the short-lived CLI calls):

   ```
   runuser -u cua -- bash calc.sh fullreset   # daemon + a11y + launch galculator
   runuser -u cua -- bash calc.sh doctor       # AT-SPI/org.a11y.Bus probe + dbus addr
   runuser -u cua -- bash calc.sh state        # AT-SPI tree (shows degraded flag if empty)
   runuser -u cua -- bash calc.sh prep         # resolve window id (needed before pxclick)
   runuser -u cua -- bash calc.sh click 7 background      # element click, bg
   runuser -u cua -- bash calc.sh pxclick 3 background    # pixel/vision click, bg
   runuser -u cua -- bash calc.sh type 789 foreground     # foreground EWMH type
   runuser -u cua -- bash calc.sh btf                     # bring_to_front (EWMH)
   ```

## Gotchas (learned the hard way)

- **AT-SPI needs the session bus.** A daemon started outside the desktop
  session has no `DBUS_SESSION_BUS_ADDRESS` → the AT-SPI tree comes back empty
  and `get_window_state` reports `degraded:true`. The daemon now auto-discovers
  it (`platform-linux/src/session_bus.rs`); if it still can't, the session has
  no a11y bus or the daemon isn't running as the desktop user.
- **Run as the desktop user.** Root-against-a-user-session can't read that
  user's session-process environ or `/run/user/<uid>/bus`. This is the Linux
  analogue of the Windows Session 0 isolation problem.
- **Zombie children pollute `pgrep`.** A single-instance app (galculator)
  launched by the daemon can leave an unreaped zombie child after a kill; it
  shows in `pgrep` and makes `list_windows` look empty. `fullreset` restarts
  the daemon, which reaps it.
- **Stale daemon → stale schema.** A long-lived daemon proxies a cached tool
  schema. To assert the *freshly built* binary's schema, shell
  `cua-driver describe <tool>` (computes the ToolDef locally) instead of a
  daemon round-trip — this is what `modality_dispatch_linux_test.rs` does.
- **VNC screenshots over the computer-server `/cmd` SSE endpoint** are `data:`-
  prefixed JSON; strip the `data: ` prefix before `json.loads`.

## CI coverage

The dispatch *contract* (every input tool advertises `delivery_mode`;
`bring_to_front` is a real EWMH activation) is asserted display-free in
`rust/crates/cua-driver/tests/modality_dispatch_linux_test.rs`. The
session-bus discovery matching logic is unit-tested in
`platform-linux/src/session_bus.rs`. This container lane covers the parts that
genuinely need a live desktop + a11y bus, and is run manually / against the
`trycua/cua-xfce` rig.
