#!/bin/bash
# The xcode tier: full plus one pinned Xcode, its iOS simulator runtime and
# the Metal toolchain. Runs IN the guest as the desktop user, on a clone of
# the full VM that just passed the doctor.
#
#   tier-xcode.sh STAGE_DIR
#
# STAGE_DIR/cache/$XCODE_XIP comes from the runner's Xcode cache (build.sh
# checks its SHA-1 on the host; checked again here, and xip verifies Apple's
# signature while it expands). Xcode lands at /Applications/Xcode_<v>.app,
# selected, licensed and first-launched. The simulator runtime and the Metal
# toolchain come from Apple through xcodebuild.
set -euo pipefail
STAGE="${1:?staging directory}"
# shellcheck source=/dev/null # STAGE/versions.env, a copy of ../versions.env
. "$STAGE/versions.env"
# The password on a here-string, not a pipe: with sudo's credential cache
# warm, sudo never reads it, and a printf pipe then dies of SIGPIPE, which
# pipefail turns into a failed step.
sudo_run() { sudo -S -p '' "$@" <<<"${CUA_SUDO_PW:-lume}"; }
say() { echo "==> $*"; }
APP="/Applications/Xcode_${XCODE_VERSION}.app"

xip_file="$STAGE/cache/$XCODE_XIP"
[ -f "$xip_file" ] || { echo "missing $xip_file" >&2; exit 1; }
say "checking $XCODE_XIP"
[ "$(shasum -a 1 "$xip_file" | cut -d' ' -f1)" = "$XCODE_XIP_SHA1" ] ||
    { echo "checksum mismatch: $xip_file" >&2; exit 1; }

say "expanding Xcode $XCODE_VERSION"
W="$(mktemp -d /tmp/cua-build-xcode.XXXXXX)"
trap 'rm -rf "$W"' EXIT
(cd "$W" && xip --expand "$xip_file" >/dev/null)
sudo_run rm -rf "$APP"
sudo_run mv "$W/Xcode.app" "$APP"
sudo_run xcode-select --switch "$APP/Contents/Developer"
sudo_run xcodebuild -license accept
sudo_run xcodebuild -runFirstLaunch
got="$(xcodebuild -version | sed -n 's/^Build version //p')"
[ "$got" = "$XCODE_BUILD" ] || { echo "Xcode build is $got, not $XCODE_BUILD" >&2; exit 1; }

say "iOS $XCODE_IOS_RUNTIME simulator runtime and Metal toolchain"
xcodebuild -downloadPlatform iOS -buildVersion "$XCODE_IOS_RUNTIME"
xcodebuild -downloadComponent MetalToolchain
# The first boot of a new runtime builds its dyld shared cache; do it now so
# the first `simctl boot` in a sandbox is not minutes long.
xcrun simctl runtime dyld_shared_cache update --all >/dev/null 2>&1 || true
xcrun simctl delete unavailable || true

say "runtimes"
xcrun simctl list runtimes
rm -rf "$HOME/Library/Caches/com.apple.dt.Xcode" "$HOME/Library/Developer/Xcode/DerivedData"
echo "xcode tier installed"
