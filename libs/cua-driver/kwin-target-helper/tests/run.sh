#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Opt-in test of the helper's input gate in a headless KWin inside a container.
# It never touches the desktop session: KWin, D-Bus and the windows live and die
# with the container.
#
#   tests/run.sh                 # gate suite: the helper on its own
#   tests/run.sh path/to/cua-driver   # also the driver suite (a portal-input build
#                                     # whose glibc the image has)
#   DISTRO=debian13 tests/run.sh ...  # on Debian 13 (KWin 6.3) instead of
#   DISTRO=fedora45 tests/run.sh ...  # Ubuntu 26.04 (KWin 6.6), or Fedora 45
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
helper="$(dirname "$here")"
engine="${CONTAINER_ENGINE:-$(command -v podman || command -v docker || true)}"
[ -n "$engine" ] || { echo "run.sh needs podman or docker" >&2; exit 2; }
image=cua-kwin-target-helper-test${DISTRO:+-$DISTRO}
"$engine" build -q -t "$image" -f "$here/Containerfile${DISTRO:+.$DISTRO}" "$here" >/dev/null

args=(--rm -v "$helper:/helper:ro,Z")
if [ $# -gt 0 ]; then
  args+=(-v "$(realpath "$1"):/driver/cua-driver:ro,Z")
fi
# KWin renders window captures on the GPU; the driver suite reads one before
# its pixel click.
if [ -e /dev/dri ]; then
  args+=(--device /dev/dri --group-add keep-groups)
fi
exec "$engine" run "${args[@]}" -e LANG=C.UTF-8 "$image" \
  dbus-run-session -- bash /helper/tests/session.sh
