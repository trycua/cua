#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# A dry run of the release workflow's Sparkle appcast, with the real key:
# two fake releases of a built "Cua Spaces.app" (a beta, 0.0.1-dryrun.N,
# then a stable 0.0.2), each packed into a disk image, signed and added to
# the appcast by scripts/make-appcast.sh; then scripts/verify-appcast.sh
# checks both items in the final feed (channel, versions, length, and the
# EdDSA signature with the app's SUPublicEDKey and Sparkle's sign_update).
# Nothing is uploaded. CI runs it when CUA_SPACES_SPARKLE_ED_PRIVATE_KEY is
# set; it proves the secret matches the public key the app ships.
#
#   CUA_SPACES_SPARKLE_ED_PRIVATE_KEY=... scripts/appcast-dry-run.sh "path/Cua Spaces.app"
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
app="${1:?usage: appcast-dry-run.sh <path/Cua Spaces.app>}"
[ -n "${CUA_SPACES_SPARKLE_ED_PRIVATE_KEY:-}" ] || { echo "CUA_SPACES_SPARKLE_ED_PRIVATE_KEY is not set" >&2; exit 2; }
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
run="${GITHUB_RUN_NUMBER:-1}"
base="https://example.invalid/cua-spaces-dry-run"

release() { # release VERSION BUILD -> the disk image
  local stage="$work/stage-$1"
  mkdir -p "$stage"
  ditto "$app" "$stage/Cua Spaces.app"
  local plist="$stage/Cua Spaces.app/Contents/Info.plist"
  /usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString ${1%%-*}" -c "Set :CFBundleVersion $2" "$plist"
  /usr/libexec/PlistBuddy -c "Delete :CuaVersion" "$plist" 2>/dev/null || true
  /usr/libexec/PlistBuddy -c "Add :CuaVersion string $1" "$plist"
  codesign --force --sign - --identifier com.trycua.spaces.macos "$stage/Cua Spaces.app" 2>/dev/null
  "$here/scripts/package-dmg.sh" --app "$stage/Cua Spaces.app" --out "$work/dmg/cua-spaces-$1-darwin-universal.dmg" >/dev/null
  echo "$work/dmg/cua-spaces-$1-darwin-universal.dmg"
}

beta="0.0.1-dryrun.$run"
stable="0.0.2"
beta_dmg="$(release "$beta" "0.0.1.$run")"
stable_dmg="$(release "$stable" "0.0.2.$run")"
printf '## %s\n\n- Dry run.\n' "$beta" > "$work/beta.md"
printf '## %s\n\n- Dry run.\n' "$stable" > "$work/stable.md"
"$here/scripts/make-appcast.sh" --dmg "$beta_dmg" --version "$beta" --download-base "$base/v$beta/" \
  --notes "$work/beta.md" --out "$work/appcast-1.xml"
"$here/scripts/make-appcast.sh" --dmg "$stable_dmg" --version "$stable" --download-base "$base/v$stable/" \
  --previous "$work/appcast-1.xml" --notes "$work/stable.md" --out "$work/appcast.xml"
# Both items, in the merged feed.
"$here/scripts/verify-appcast.sh" --appcast "$work/appcast.xml" --dmg "$beta_dmg" \
  --url "$base/v$beta/$(basename "$beta_dmg")" --version "$beta"
"$here/scripts/verify-appcast.sh" --appcast "$work/appcast.xml" --dmg "$stable_dmg" \
  --url "$base/v$stable/$(basename "$stable_dmg")" --version "$stable"
echo "appcast dry run passed: $(grep -c '<item>' "$work/appcast.xml") items"
