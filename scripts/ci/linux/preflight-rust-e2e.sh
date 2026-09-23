#!/usr/bin/env bash
# Read-only readiness checks before starting the Linux desktop matrix.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
DRIVER_BIN="${CUA_TEST_DRIVER_BIN:-${REPO_ROOT}/libs/cua-driver/rust/target/release/cua-driver}"
FAILURES=0

pass() { printf '[ready] %s\n' "$1"; }
warn() { printf '[optional] %s\n' "$1"; }
fail() { printf '[missing] %s\n' "$1" >&2; FAILURES=$((FAILURES + 1)); }

if [[ "$(uname -s)" != Linux ]]; then
  fail 'Linux host required'
else
  pass "Linux architecture: $(uname -m)"
fi

if [[ -n "${WAYLAND_DISPLAY:-}" ]]; then
  display_socket="${WAYLAND_DISPLAY}"
  if [[ "${display_socket}" != /* ]]; then
    display_socket="${XDG_RUNTIME_DIR:-/nonexistent}/${display_socket}"
  fi
  if [[ -S "${display_socket}" ]]; then
    pass "Wayland display socket: ${display_socket}"
  else
    fail "Wayland display socket unavailable: ${display_socket}"
  fi
  pass "Session/compositor: ${XDG_SESSION_TYPE:-unknown}/${XDG_CURRENT_DESKTOP:-unknown}; verify compositor-specific identity in the strict matrix preflight"
  if [[ -z "${DISPLAY:-}" ]]; then
    for tool in wf-recorder grim wtype; do
      if command -v "${tool}" >/dev/null 2>&1; then pass "${tool} available"; else fail "${tool} required for native Wayland"; fi
    done
  fi
fi

if [[ -n "${DISPLAY:-}" ]]; then
  if command -v xdpyinfo >/dev/null 2>&1; then
    if xdpyinfo -display "${DISPLAY}" >/dev/null 2>&1; then
      pass "X11 display reachable: ${DISPLAY}"
    else
      fail "X11 display not reachable: ${DISPLAY} (check Xauthority and session)"
    fi
  else
    warn 'xdpyinfo unavailable; X11 reachability is unverified until the strict matrix preflight'
  fi
  if command -v ffmpeg >/dev/null 2>&1; then pass 'ffmpeg available'; else fail 'ffmpeg required for X11 recording'; fi
fi

if [[ -z "${DISPLAY:-}" && -z "${WAYLAND_DISPLAY:-}" ]]; then
  fail 'no DISPLAY or WAYLAND_DISPLAY; enter the desktop session or use xvfb-run with dbus-run-session'
fi

for tool in ffprobe jq; do
  if command -v "${tool}" >/dev/null 2>&1; then pass "${tool} available"; else fail "${tool} required for E2E"; fi
done
if [[ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]]; then
  fail 'no session D-Bus address; enter the desktop user session or use dbus-run-session'
elif command -v gdbus >/dev/null 2>&1; then
  if gdbus call --session --dest org.freedesktop.DBus --object-path /org/freedesktop/DBus \
      --method org.freedesktop.DBus.ListNames >/dev/null 2>&1; then
    pass 'session D-Bus reachable'
  else
    fail 'session D-Bus unreachable; enter the desktop user session or use dbus-run-session'
  fi
else
  warn 'gdbus unavailable; session D-Bus address exists but reachability is unverified'
fi

source_sha="$(git -C "${REPO_ROOT}" rev-parse HEAD 2>/dev/null || true)"
if [[ "${source_sha}" =~ ^[0-9a-f]{40}$ ]]; then
  pass "checked-out source SHA: ${source_sha}"
else
  fail 'checked-out source SHA unavailable'
fi
if [[ -n "${CUA_E2E_SOURCE_SHA:-}" && "${CUA_E2E_SOURCE_SHA,,}" != "${source_sha}" ]]; then
  fail 'CUA_E2E_SOURCE_SHA differs from the checkout; sync exact source before testing'
fi
if [[ -x "${DRIVER_BIN}" ]]; then
  if version="$("${DRIVER_BIN}" --version 2>&1)"; then
    pass "driver version: ${version} (${DRIVER_BIN}); source identity not yet verified"
  else
    fail "source driver cannot run --version: ${DRIVER_BIN}"
  fi
else
  warn "source driver not built: ${DRIVER_BIN}"
fi

for browser in google-chrome chromium microsoft-edge; do
  if command -v "${browser}" >/dev/null 2>&1; then warn "browser available: ${browser}"; fi
done
warn 'fixture, AX, capture, permission, and recording checks require the strict matrix preflight'

if ((FAILURES)); then
  printf '%s required preflight check(s) failed\n' "${FAILURES}" >&2
  exit 1
fi
pass 'lightweight environment checks complete (not desktop certification)'
