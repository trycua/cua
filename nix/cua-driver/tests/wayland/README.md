# cua-driver native-Wayland TDD suite

A reproduction of the cua-driver NixOS tests on **native Wayland**, across five
desktop sessions. These tests are a **TDD red suite**: the Linux backend is
X11-only today, so they are **expected to fail** until native Wayland support is
added. They exist to *specify* that work and to show, per compositor, exactly
what survives.

## Why they fail today

cua-driver enumerates windows with X11 `_NET_CLIENT_LIST`, captures with
`import`/`xwd`/XGetImage, and injects input with X11 `XSendEvent` (plus uinput +
XI2 for the agent cursor / parallel drags). On native Wayland:

- apps are **Wayland clients with no X11 window id**, so `list_windows` returns
  nothing and `find_window` times out;
- there is no X display to capture or `XSendEvent` into (the tests never set
  `DISPLAY`, so there is no XWayland fallback either).

To make a scenario green you implement the corresponding native-Wayland path in
`libs/cua-driver/rust/crates/platform-linux` (e.g. a Wayland window enumerator,
a `grim`/screencopy capture path, and a `wlr-virtual-pointer` /
`virtual-keyboard` / `libei` input path), then re-run the matching check.

## Layout

- `session.nix` — brings up a chosen desktop headless and exposes the Wayland
  socket. The compositor is the only per-desktop difference.
- `driver-client.nix` — shared MCP client; window discovery goes **only** through
  cua-driver's own `list_windows` (no xdotool/X11 cheat).
- `record-wayland-gif.nix` — `grim`-based GIF recorder (wlroots only; no-op
  elsewhere).
- `integration.nix`, `screenshot.nix`, `cursor-click-gif.nix`,
  `background-terminal-gif.nix`, `parallel-drag.nix`, `background-gui.nix` — the
  scenarios, each parameterised by `desktop` (and `background-gui` also by `app`).

## Matrix

Desktops: `xfce-labwc`, `xfce-wayfire`, `xfce-sway` (XFCE 4.20's Wayland-session
compositors), `kde` (kwin_wayland), `gnome` (mutter headless).

Checks (built on demand):

```
nix build .#checks.x86_64-linux.cua-driver-wayland-<desktop>-<scenario>
nix build .#checks.x86_64-linux.cua-driver-wayland-<desktop>-background-gui-<app>
```

Scenarios: `integration`, `screenshot`, `cursor-click-gif`,
`background-terminal-gif`, `parallel-drag`.
Background-GUI apps: `foot`, `gtk3-gedit`, `qt6-kcalc`.

`parallel-drag` is the hardest: it needs X11 MPX master pointers, for which there
is no native Wayland equivalent — keep it red, or redesign around multi-seat /
virtual-pointer protocols.

## Running in CI

The `.github/workflows/nix-wayland.yml` workflow runs the whole matrix on the
same triggers as the X11 `nix-build.yml` workflow — PRs and `main` pushes that
touch `nix/**`, `flake.{nix,lock}`, `libs/cua-driver/rust/**`, or the workflow
itself, plus manual `workflow_dispatch`. The jobs are BLOCKING: a red cell fails
the PR, so the suite only goes green once native Wayland support lands. Each job
uploads its screenshots / GIFs / logs as artifacts so you can see what each
compositor did.
