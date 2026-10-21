#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Proves a dev-flavour "Cua Spaces Dev.app" cannot touch an installed
# production Cua Spaces: another bundle id and name, no Sparkle feed or key
# (so it can never update itself into production), staging by default, its
# own home and host service label, and a signature under the dev id.
#
#   scripts/check-dev-bundle.sh "path/Cua Spaces Dev <instance>.app"
set -euo pipefail
app="${1:?usage: check-dev-bundle.sh <path/Cua Spaces Dev <instance>.app>}"
plist="$app/Contents/Info.plist"
[ -f "$plist" ] || { echo "$app has no Contents/Info.plist" >&2; exit 1; }
fail=0
bad() { echo "dev bundle check: $1" >&2; fail=1; }
key() { /usr/libexec/PlistBuddy -c "Print :$1" "$plist" 2>/dev/null || true; }
expect() { # expect KEY VALUE
  [ "$(key "$1")" = "$2" ] || bad "$1 is '$(key "$1")', want '$2'"
}

id="$(key CFBundleIdentifier)"
[ "$id" != com.trycua.spaces.macos ] || bad "bundle id is the production one"
inst="$(key CuaInstance)"
[ -n "$inst" ] || bad "CuaInstance is not set"
[ "$id" = "com.trycua.spaces.macos.dev.$inst" ] || bad "bundle id '$id' is not com.trycua.spaces.macos.dev.$inst"
[ "$(basename "$app")" != "Cua Spaces.app" ] || bad "app is named like the production app"
expect CFBundleName "Cua Spaces Dev $inst"
expect CFBundleDisplayName "Cua Spaces Dev $inst"
for k in SUFeedURL SUPublicEDKey SUEnableAutomaticChecks; do
  [ -z "$(key "$k")" ] || bad "$k is set: a dev build could update itself into production"
done
expect CuaEnv staging
expect CuaHome "~/.cua-dev/$inst"
expect CuaServiceSuffix "dev-$inst"
signed="$(codesign -dv "$app" 2>&1 | sed -n 's/^Identifier=//p')"
[ "$signed" = "$id" ] || bad "signed as '$signed', want '$id'"
[ "$fail" = 0 ] || exit 1
echo "dev bundle ok: $id"
