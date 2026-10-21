#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Runs a dev build ("Cua Spaces Dev <instance>.app") beside the production app
# and other dev instances: Cua staging, its own home (~/.cua-dev/<instance>)
# and its own host service label. The instance is CUA_INSTANCE or this
# worktree's name, as in build-app.sh. The
# bundle already carries these (build-app.sh); setting them here as well
# keeps a shell-launched app and any `cua` it starts in the same place.
#
#   scripts/run-dev.sh ["path/Cua Spaces Dev <instance>.app"]
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
app="${1:-}"
[ -n "$app" ] || app="$(ls -d "$here"/.build/app/"Cua Spaces Dev "*.app 2>/dev/null | head -n 1)"
[ -d "$app" ] || { echo "no $app: run scripts/build-app.sh first" >&2; exit 1; }
"$here/scripts/check-dev-bundle.sh" "$app" >&2
export CUA_ENV="${CUA_ENV:-staging}"
inst="$(/usr/libexec/PlistBuddy -c "Print :CuaInstance" "$app/Contents/Info.plist")"
export CUA_HOME="${CUA_HOME:-$HOME/.cua-dev/$inst}"
export CUA_SERVICE_SUFFIX="${CUA_SERVICE_SUFFIX:-dev-$inst}"
exec "$app/Contents/MacOS/CuaSpacesMac"
