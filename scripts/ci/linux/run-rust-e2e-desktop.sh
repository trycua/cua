#!/usr/bin/env bash
# Validate the canonical Rust matrix in a representative maintainer desktop.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENVIRONMENT="${1:-}"
if [[ $# -gt 0 ]]; then
  shift
fi

case "${ENVIRONMENT}" in
  gnome)
    [[ "${XDG_SESSION_TYPE:-}" == wayland ]] || {
      echo "GNOME validation requires an active Wayland user session" >&2
      exit 2
    }
    [[ "${XDG_CURRENT_DESKTOP:-}" == *GNOME* ]] || {
      echo "XDG_CURRENT_DESKTOP does not identify GNOME: ${XDG_CURRENT_DESKTOP:-<unset>}" >&2
      exit 2
    }
    gdbus call --session \
      --dest org.cua.WinRects \
      --object-path /org/cua/WinRects \
      --method org.cua.WinRects.GetRects >/dev/null || {
        echo "The GNOME WinRects helper is not available in this user session" >&2
        exit 2
      }
    export CUA_E2E_COMPOSITOR=gnome-mutter
    export CUA_E2E_INPUT_BACKENDS=atspi,libei-portal
    export CUA_DRIVER_RS_ENABLE_WAYLAND=1
    ;;
  kde)
    [[ "${XDG_SESSION_TYPE:-}" == wayland ]] || {
      echo "KDE validation requires an active Wayland user session" >&2
      exit 2
    }
    [[ "${XDG_CURRENT_DESKTOP:-}" == *KDE* ]] || {
      echo "XDG_CURRENT_DESKTOP does not identify KDE: ${XDG_CURRENT_DESKTOP:-<unset>}" >&2
      exit 2
    }
    kwin_version="$(kwin_wayland --version 2>/dev/null | sed -n 's/^kwin \([0-9][0-9]*\).*/\1/p')"
    [[ "${kwin_version:-0}" -ge 6 ]] || {
      echo "Representative KDE validation requires Plasma/KWin 6; found ${kwin_version:-unknown}" >&2
      exit 2
    }
    export CUA_E2E_COMPOSITOR=kwin
    export CUA_E2E_INPUT_BACKENDS=atspi,libei-portal
    export CUA_DRIVER_RS_ENABLE_WAYLAND=1
    ;;
  hyprland)
    [[ "${XDG_SESSION_TYPE:-}" == wayland ]] || {
      echo "Hyprland validation requires an active Wayland user session" >&2
      exit 2
    }
    [[ "${XDG_CURRENT_DESKTOP,,}" == *hyprland* ]] || {
      echo "XDG_CURRENT_DESKTOP does not identify Hyprland: ${XDG_CURRENT_DESKTOP:-<unset>}" >&2
      exit 2
    }
    [[ -n "${HYPRLAND_INSTANCE_SIGNATURE:-}" ]] || {
      echo "HYPRLAND_INSTANCE_SIGNATURE is unavailable in this user session" >&2
      exit 2
    }
    command -v hyprctl >/dev/null || {
      echo "hyprctl is required for representative Hyprland validation" >&2
      exit 2
    }
    hyprctl -j version >/dev/null || {
      echo "hyprctl cannot query the active Hyprland compositor" >&2
      exit 2
    }
    hyprctl -j clients >/dev/null || {
      echo "hyprctl cannot query Hyprland client identity" >&2
      exit 2
    }
    command -v jq >/dev/null || {
      echo "jq is required for representative Hyprland validation" >&2
      exit 2
    }
    if [[ -z "${CUA_WAYLAND_RECORDING_OUTPUT:-}" ]]; then
      CUA_WAYLAND_RECORDING_OUTPUT="$(
        hyprctl -j monitors | jq -er '.[] | select(.focused) | .name'
      )" || {
        echo "hyprctl did not identify one focused output for video recording" >&2
        exit 2
      }
      export CUA_WAYLAND_RECORDING_OUTPUT
    fi
    export CUA_E2E_WAYLAND_SESSION=hyprland
    export CUA_E2E_COMPOSITOR=hyprland
    export CUA_E2E_INPUT_BACKENDS=atspi,wlr-virtual-pointer
    export CUA_DRIVER_RS_ENABLE_WAYLAND=1
    ;;
  xorg)
    [[ -n "${DISPLAY:-}" && "${XDG_SESSION_TYPE:-x11}" != wayland ]] || {
      echo "Real-Xorg validation requires DISPLAY in a non-Wayland session" >&2
      exit 2
    }
    export CUA_E2E_COMPOSITOR=real-xorg
    export CUA_E2E_INPUT_BACKENDS=atspi,xsend-event,xtest,mpx-uinput
    ;;
  *)
    echo "Usage: run-rust-e2e-desktop.sh {gnome|kde|hyprland|xorg} [--no-build]" >&2
    exit 2
    ;;
esac

effective_harness_filter="${CUA_E2E_HARNESS_FILTER:-electron,tauri}"
if [[ ",${effective_harness_filter}," == *,tauri,* && ! -e /dev/dri/renderD128 ]]; then
  echo "Tauri/WebKitGTK validation requires a representative DRM render node" >&2
  exit 2
fi

DESKTOP_RUNNER="${CUA_E2E_DESKTOP_RUNNER:-${SCRIPT_DIR}/run-rust-e2e.sh}"
if [[ ! -x "${DESKTOP_RUNNER}" ]]; then
  echo "Linux desktop session runner is not executable: ${DESKTOP_RUNNER}" >&2
  exit 2
fi
exec "${DESKTOP_RUNNER}" "$@"
