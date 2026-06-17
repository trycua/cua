# Shared Wayland desktop-session bring-up for the cua-driver Wayland suite.
#
# Two execution models, selected per desktop:
#
#   * NATIVE (wlroots: labwc, sway) — apps run as real Wayland clients of the
#     HOST compositor, and the driver talks to that same compositor's wlr
#     protocols (foreign-toplevel + screencopy). The session `start` script
#     launches the compositor headless, waits for its socket, and publishes it
#     to /tmp/wl-display.
#
#   * NESTED (kde: kwin, gnome: mutter) — kwin/mutter expose NO client protocols
#     for cross-window enumeration/capture, so the driver cannot drive them
#     directly. Instead cua-driver "brings its own compositor": with
#     CUA_WAYLAND_NEST=1 it spawns a private headless labwc and points
#     WAYLAND_DISPLAY at it, so every app launched via `launch_app` runs inside
#     that nested session where the wlr protocols DO work. The host kwin/mutter
#     is still booted (best-effort) to prove cua-driver coexists with a real
#     KDE/GNOME host, but the test never depends on it.
#
# Given a `desktop` this returns:
#
#   { packages; start; label; nested; driverWrapper; }
#
#   * packages      — extra `environment.systemPackages` the session needs.
#   * start         — a `writeShellScript` that brings the session up and
#                     `touch`es /tmp/wl-ready, then blocks.
#   * label         — human label for logs.
#   * nested        — true for kde/gnome (driver hosts its own compositor).
#   * driverWrapper — a `writeShellScript` to use as CUA_DRIVER_BIN: it sets the
#                     correct env (host WAYLAND_DISPLAY for native; the nest env
#                     for nested) and execs `cua-driver "$@"`. Tests spawn the
#                     driver through this so the python harness stays
#                     desktop-agnostic.
{
  pkgs,
  desktop,
}:

let
  inherit (pkgs) lib;

  # Native-Wayland tooling only. No xorg.* / xdotool / xterm here — those are
  # X11 and would let the driver cheat via XWayland. `foot` is a Wayland-native
  # terminal; `grim` captures the wlroots output for the GIF artifacts.
  commonPkgs = with pkgs; [
    foot
    grim
    wtype # virtual-keyboard CLI: cua-driver shells out to it for type_text/press_key
    dbus
    procps
  ];

  # Minimal sway config: native Wayland only (XWayland off), stable output.
  # `workspace_layout stacking` makes the focused window fill the output, so the
  # driver's centre virtual-pointer click reliably lands on the activated target
  # (sway needs a pointer interaction to route virtual-keyboard input on a
  # headless seat with no physical keyboard).
  swayConfig = pkgs.writeText "sway-config" ''
    xwayland disable
    output HEADLESS-1 resolution 1280x1024
    workspace_layout stacking
    exec_always foot --server
  '';

  desktops = {
    "xfce-labwc" = {
      label = "XFCE on labwc (native Wayland)";
      packages = with pkgs; [ labwc ];
      wlroots = true;
      nested = false;
      launch = "labwc >/tmp/compositor.log 2>&1 &";
    };
    # NOTE: xfce-wayfire intentionally omitted — wayfire fails to build in the
    # current nixpkgs pin (wf-config can't link -ldoctest). labwc + sway cover
    # XFCE-on-wlroots.
    "xfce-sway" = {
      label = "XFCE on sway (native Wayland)";
      packages = with pkgs; [ sway ];
      wlroots = true;
      nested = false;
      launch = "sway -c ${swayConfig} >/tmp/compositor.log 2>&1 &";
    };
    "kde" = {
      label = "KDE Plasma kwin_wayland host + cua-driver nested labwc";
      # labwc is what cua-driver nests; kwin is the host it coexists with.
      packages = with pkgs; [ labwc kdePackages.kwin ];
      wlroots = false;
      nested = true;
      # No --xwayland: keep the host session native-Wayland only.
      launch = "kwin_wayland --virtual --width 1280 --height 1024 --no-lockscreen >/tmp/compositor.log 2>&1 &";
    };
    "gnome" = {
      label = "GNOME Shell / mutter host + cua-driver nested labwc";
      packages = with pkgs; [ labwc gnome-shell mutter ];
      wlroots = false;
      nested = true;
      launch = "gnome-shell --wayland --headless --virtual-monitor 1280x1024 --unsafe-mode >/tmp/compositor.log 2>&1 &";
    };
  };

  cfg = desktops.${desktop} or (throw "wayland/session.nix: unknown desktop '${desktop}'");

  wlrootsEnv = lib.optionalString cfg.wlroots ''
    export WLR_BACKENDS=headless
    export WLR_RENDERER=pixman
    export WLR_RENDERER_ALLOW_SOFTWARE=1
    export WLR_LIBINPUT_NO_DEVICES=1
  '';

  # Native model: block until the HOST compositor's socket appears, publish it.
  nativeStart = ''
    dbus-run-session -- sh -c '
      wlsock() { ls "$XDG_RUNTIME_DIR" 2>/dev/null | grep -E "^wayland-[0-9]+$" | head -1; }

      ${cfg.launch}
      compositor_pid=$!

      n=0
      while [ -z "$(wlsock)" ] && [ "$n" -lt 120 ]; do
        if ! kill -0 "$compositor_pid" 2>/dev/null; then
          echo "compositor exited early" >&2
          break
        fi
        sleep 0.5
        n=$((n + 1))
      done

      sock="$(wlsock)"
      if [ -z "$sock" ]; then
        echo "no Wayland display socket appeared" >&2
        cat /tmp/compositor.log >&2 2>/dev/null || true
        exit 1
      fi
      echo "$sock" > /tmp/wl-display

      touch /tmp/wl-ready
      exec sleep infinity
    '
  '';

  # Nested model: boot the host compositor BEST-EFFORT (log whether it came up),
  # but never block the test on it — cua-driver provides its own session. We
  # record the host socket (if any) for logs; the driver wrapper ignores it.
  nestedStart = ''
    dbus-run-session -- sh -c '
      wlsock() { ls "$XDG_RUNTIME_DIR" 2>/dev/null | grep -E "^wayland-[0-9]+$" | head -1; }

      ${cfg.launch}

      n=0
      while [ -z "$(wlsock)" ] && [ "$n" -lt 40 ]; do
        sleep 0.5
        n=$((n + 1))
      done
      sock="$(wlsock)"
      if [ -n "$sock" ]; then
        echo "host compositor up: $sock" >&2
      else
        echo "host compositor did not come up; proceeding with nested session only" >&2
        cat /tmp/compositor.log >&2 2>/dev/null || true
      fi
      echo "''${sock:-none}" > /tmp/wl-display

      touch /tmp/wl-ready
      exec sleep infinity
    '
  '';

  start = pkgs.writeShellScript "wayland-session-start-${desktop}.sh" ''
    set -u
    export PATH=${lib.makeBinPath (commonPkgs ++ cfg.packages)}:$PATH

    export XDG_RUNTIME_DIR=/run/user/0
    mkdir -p "$XDG_RUNTIME_DIR"
    chmod 700 "$XDG_RUNTIME_DIR"

    ${wlrootsEnv}
    export QT_QPA_PLATFORM=wayland
    export GDK_BACKEND=wayland
    export LIBGL_ALWAYS_SOFTWARE=1

    rm -f /tmp/wl-ready /tmp/wl-display "$XDG_RUNTIME_DIR/.cua-nested-display"

    ${if cfg.nested then nestedStart else nativeStart}
  '';

  # Driver launcher used as CUA_DRIVER_BIN. It owns the WAYLAND env so the python
  # harness never has to. labwc/foot/grim are on PATH for the app/compositor
  # processes cua-driver spawns.
  # Toolkit apps cua-driver launches inherit its env, so force the NATIVE
  # Wayland backends here (GTK auto-detects from WAYLAND_DISPLAY; Qt would
  # otherwise default to xcb and fail with no X). Harmless for foot.
  appBackendEnv = ''
    export GDK_BACKEND=wayland
    export QT_QPA_PLATFORM=wayland
    export LIBGL_ALWAYS_SOFTWARE=1
  '';

  driverWrapper =
    if cfg.nested then
      pkgs.writeShellScript "cua-driver-nested-${desktop}" ''
        export PATH=${lib.makeBinPath (commonPkgs ++ cfg.packages)}:$PATH
        export XDG_RUNTIME_DIR=/run/user/0
        export CUA_WAYLAND_NEST=1
        export CUA_WAYLAND_NEST_COMPOSITOR=labwc
        ${appBackendEnv}
        unset DISPLAY WAYLAND_DISPLAY
        exec cua-driver "$@"
      ''
    else
      pkgs.writeShellScript "cua-driver-native-${desktop}" ''
        export PATH=${lib.makeBinPath (commonPkgs ++ cfg.packages)}:$PATH
        export XDG_RUNTIME_DIR=/run/user/0
        export WAYLAND_DISPLAY="$(cat /tmp/wl-display)"
        ${appBackendEnv}
        unset DISPLAY
        exec cua-driver "$@"
      '';
in
{
  inherit (cfg) label nested;
  packages = commonPkgs ++ cfg.packages;
  inherit start driverWrapper;
}
