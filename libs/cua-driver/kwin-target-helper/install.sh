#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Builds the helper against this machine's KWin, installs it where KWin looks
# for effects, and has cua-driver turn it on. Run it again after a KWin upgrade:
# KWin loads only a helper built for its own version.
#
#   install.sh              # build, install (asks for root), turn on
#   install.sh --uninstall  # turn off and remove
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
plugin=kwin/effects/plugins/cua_kwin_target_helper.so

as_root() {
  if [ "$(id -u)" = 0 ]; then "$@"
  elif command -v pkexec >/dev/null && [ -n "${WAYLAND_DISPLAY:-}${DISPLAY:-}" ]; then pkexec "$@"
  else sudo "$@"
  fi
}

# cua-driver from PATH, or from CUA_DRIVER.
driver="${CUA_DRIVER:-$(command -v cua-driver || true)}"
switch() {
  if [ -n "$driver" ]; then
    "$driver" kwin-capture "$1"
  else
    echo "cua-driver is not on PATH; run \`cua-driver kwin-capture $1\` yourself." >&2
  fi
}

if [ "${1:-}" = --uninstall ]; then
  switch off
  for dir in /usr/lib/*/qt6/plugins /usr/lib/qt6/plugins /usr/lib64/qt6/plugins; do
    if [ -e "$dir/$plugin" ]; then as_root rm "$dir/$plugin"; fi
  done
  exit
fi

build="$(mktemp -d)"
trap 'rm -rf "$build"' EXIT
# /usr, because KWin doesn't search /usr/local for plugins.
cmake -S "$here" -B "$build" -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr >/dev/null
cmake --build "$build" --parallel >/dev/null
as_root cmake --install "$build"
switch on
