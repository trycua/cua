#!/usr/bin/env bash
# Only asserted when the driver is actually installed: an image built with
# CUA_SPACESD_SOURCE=none is still a healthy desktop.
set -euo pipefail
[ -x /usr/local/bin/cua-spacesd ] || [ -x /usr/local/bin/cua-guestd ] || [ -x /usr/local/bin/cua-env-driver ] || exit 0
ss -Hltn "sport = :3211" | grep -q .
