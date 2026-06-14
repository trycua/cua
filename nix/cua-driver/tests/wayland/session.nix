# Shared NATIVE-Wayland desktop-session bring-up for the cua-driver Wayland TDD
# suite.
#
# These tests deliberately target *native* Wayland — apps run as real Wayland
# clients (NOT XWayland), and the tests never set DISPLAY, so the X11-only
# cua-driver has nothing to fall back to. Today the driver therefore cannot see
# or drive these windows and the whole suite is RED; it is a TDD specification
# for adding native Wayland support to the Linux backend.
#
# Given a `desktop` this returns:
#
#   { packages; start; label; }
#
#   * packages — extra `environment.systemPackages` the session needs.
#   * start    — a `writeShellScript` that launches the compositor headless,
#                waits for its Wayland socket, writes the socket name to
#                /tmp/wl-display, then `touch`es /tmp/wl-ready and blocks. Tests
#                run it in the background, wait for /tmp/wl-ready, read
#                WAYLAND_DISPLAY, and drive cua-driver against native clients.
#   * label    — human label for logs.
#
# Supported desktops (the compositor IS the per-desktop difference):
#   xfce-labwc   — labwc, the compositor XFCE 4.20's Wayland session uses
#   xfce-wayfire — the other compositor XFCE 4.20's Wayland session supports
#   xfce-sway    — sway (wlroots, headless-friendly)
#   kde          — KDE Plasma's kwin_wayland (virtual backend)
#   gnome        — GNOME Shell / mutter headless (virtual monitor)
#
# Everything runs HEADLESS (no GPU/seat): wlroots compositors use the headless
# backend + pixman software renderer; kwin uses --virtual; mutter uses
# --headless --virtual-monitor.
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
    dbus
    procps
  ];

  # Minimal sway config: native Wayland only (XWayland off), stable output.
  swayConfig = pkgs.writeText "sway-config" ''
    xwayland disable
    output HEADLESS-1 resolution 1280x1024
    exec_always foot --server
  '';

  # Minimal wayfire config: XWayland disabled so apps are forced native.
  wayfireConfig = pkgs.writeText "wayfire.ini" ''
    [core]
    plugins = autostart
    xwayland = false

    [autostart]
    autostart_wf_shell = false
  '';

  desktops = {
    "xfce-labwc" = {
      label = "XFCE on labwc (native Wayland)";
      packages = with pkgs; [ labwc ];
      wlroots = true;
      launch = "labwc >/tmp/compositor.log 2>&1 &";
    };
    "xfce-wayfire" = {
      label = "XFCE on wayfire (native Wayland)";
      packages = with pkgs; [ wayfire ];
      wlroots = true;
      launch = "wayfire -c ${wayfireConfig} >/tmp/compositor.log 2>&1 &";
    };
    "xfce-sway" = {
      label = "XFCE on sway (native Wayland)";
      packages = with pkgs; [ sway ];
      wlroots = true;
      launch = "sway -c ${swayConfig} >/tmp/compositor.log 2>&1 &";
    };
    "kde" = {
      label = "KDE Plasma kwin_wayland (native Wayland)";
      packages = with pkgs; [ kdePackages.kwin ];
      wlroots = false;
      # No --xwayland: keep the session native-Wayland only.
      launch = "kwin_wayland --virtual --width 1280 --height 1024 --no-lockscreen >/tmp/compositor.log 2>&1 &";
    };
    "gnome" = {
      label = "GNOME Shell / mutter (native Wayland, headless)";
      packages = with pkgs; [ gnome-shell mutter ];
      wlroots = false;
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

    rm -f /tmp/wl-ready /tmp/wl-display

    # Run the compositor + readiness wait inside the session bus. `wlsock` is
    # defined INSIDE this subshell (sh -c spawns a fresh shell) and prints the
    # name of the first Wayland display socket under XDG_RUNTIME_DIR.
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
in
{
  inherit (cfg) label;
  packages = commonPkgs ++ cfg.packages;
  inherit start;
}
