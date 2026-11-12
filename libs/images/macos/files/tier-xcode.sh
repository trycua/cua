#!/bin/bash
# The xcode tier (the macOS Space with developer tools): full plus Xcode, its
# iOS simulator runtime, the Metal toolchain and the Apple developer tools
# below. Runs IN the guest as the desktop user, on a clone of the full VM
# that already passed the doctor.
#
#   tier-xcode.sh STAGE_DIR
#
# The Xcode comes from the first of:
#   STAGE/cache/$XCODE_XIP   the pinned archive of CI's runner (build.sh); its
#                            SHA-1 is checked here and the build must be
#                            $XCODE_BUILD
#   STAGE/xcode/*.zip|*.app  an Xcode already installed on the host Mac (a ditto
#                            zip); with STAGE/xcode/pinned (build.sh, the pinned
#                            build) the pin checks apply, else any Xcode is taken
#   STAGE/xcode/*.xip        an archive the user downloaded (derive-local.sh);
#                            xip checks Apple's signature while it expands
# and lands at /Applications/Xcode_<version>.app, selected, licensed and
# first-launched. No sign-in is involved: the Xcode is one that is already on
# the Mac (or runner) building the image.
#
# Installs, all pinned in versions.env:
#   Xcode, Metal toolchain, iOS simulator runtime   Apple (no sign-in)
#   Rust targets $RUST_IOS_TARGETS                   rustup
#   sccache, XcodeGen                                /usr/local/bin (release archives)
#   CocoaPods                                        Homebrew bottle (checked)
#   developer mode, SwiftPM macro/plugin prompts off, one warmed Simulator device
set -euo pipefail
STAGE="${1:?staging directory}"
# shellcheck source=/dev/null # STAGE/versions.env, a copy of ../versions.env
. "$STAGE/versions.env"
C="$STAGE/cache"
# The password on a here-string, not a pipe: with sudo's credential cache
# warm, sudo never reads it, and a printf pipe then dies of SIGPIPE, which
# pipefail turns into a failed step.
sudo_run() { sudo -S -p '' "$@" <<<"${CUA_SUDO_PW:-lume}"; }
say() { echo "==> $*"; }
verified() {  # URL SHA256 -> the cached file, checked
    local f; f="$C/$(basename "$1")"
    [ "$(shasum -a 256 "$f" | cut -d' ' -f1)" = "$2" ] || { echo "checksum mismatch: $f" >&2; exit 1; }
    echo "$f"
}
W="$(mktemp -d /tmp/cua-build-xcode.XXXXXX)"
trap 'rm -rf "$W"' EXIT
export PATH="$HOME/.cargo/bin:/usr/local/bin:$PATH"

# --- Xcode -------------------------------------------------------------------
src="" pinned=0
if [ -f "$C/$XCODE_XIP" ]; then
    say "checking $XCODE_XIP"
    [ "$(shasum -a 1 "$C/$XCODE_XIP" | cut -d' ' -f1)" = "$XCODE_XIP_SHA1" ] ||
        { echo "checksum mismatch: $C/$XCODE_XIP" >&2; exit 1; }
    src="$C/$XCODE_XIP" pinned=1
else
    # An Xcode build.sh staged from the runner's own installed Xcode (the same
    # pinned build): the pin checks apply to it too.
    [ ! -e "$STAGE/xcode/pinned" ] || pinned=1
    for f in "$STAGE"/xcode/*.zip "$STAGE"/xcode/*.app "$STAGE"/xcode/*.xip; do
        [ -e "$f" ] && { src="$f"; break; }
    done
fi
[ -n "$src" ] || { echo "no Xcode to install: expected $C/$XCODE_XIP or an Xcode.app or .xip in $STAGE/xcode" >&2; exit 1; }

case "$src" in
    *.xip)
        say "expanding $(basename "$src") (xip checks Apple's signature)"
        (cd "$W" && xip --expand "$src" >/dev/null)
        app="$(ls -d "$W"/Xcode*.app | head -1)"
        version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist")"
        APP="/Applications/Xcode_${version}.app"
        sudo_run rm -rf "$APP"
        sudo_run mv "$app" "$APP" ;;
    *.zip)
        # The host's own Xcode, archived by ditto (links, signature, xattrs kept).
        say "extracting $(basename "$src")"
        sudo_run ditto -x -k "$src" "$W"
        app="$(ls -d "$W"/Xcode*.app | head -1)"
        version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist")"
        APP="/Applications/Xcode_${version}.app"
        sudo_run rm -rf "$APP"
        sudo_run mv "$app" "$APP" ;;
    *)
        # A bundle copy straight into place: ditto keeps the code signature,
        # links and xattrs, and the source is only read.
        version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$src/Contents/Info.plist")"
        APP="/Applications/Xcode_${version}.app"
        say "copying $(basename "$src") to $APP"
        sudo_run rm -rf "$APP"
        sudo_run ditto "$src" "$APP" ;;
esac
say "Xcode $version at $APP"
codesign --verify --deep --strict "$APP" 2>&1 | head -3 || true
sudo_run xcode-select --switch "$APP/Contents/Developer"
sudo_run xcodebuild -license accept
sudo_run xcodebuild -runFirstLaunch
build="$(xcodebuild -version | sed -n 's/^Build version //p')"
if [ "$pinned" = 1 ] && [ "$build" != "$XCODE_BUILD" ]; then
    echo "Xcode build is $build, not $XCODE_BUILD" >&2; exit 1
fi
echo "   Xcode $version ($build)"

# --- simulator runtime and Metal toolchain --------------------------------------
# Disk first: the runtime is several GB and the build volume is the guest's.
free_gb="$(df -g / | awk 'NR==2 {print $4}')"
[ "${free_gb:-0}" -ge 20 ] || { echo "the guest has ${free_gb} GB free; the iOS runtime needs about 20" >&2; exit 1; }
if [ "$pinned" = 1 ]; then
    say "iOS $XCODE_IOS_RUNTIME simulator runtime and Metal toolchain"
    xcodebuild -downloadPlatform iOS -buildVersion "$XCODE_IOS_RUNTIME"
else
    say "iOS simulator runtime of Xcode $version and Metal toolchain"
    xcodebuild -downloadPlatform iOS
fi
xcodebuild -downloadComponent MetalToolchain
# The first boot of a new runtime builds its dyld shared cache; do it now so
# the first `simctl boot` in a Space is not minutes long.
xcrun simctl runtime dyld_shared_cache update --all >/dev/null 2>&1 || true
xcrun simctl delete unavailable || true

# --- developer tools ---------------------------------------------------------------
say "Rust targets: $RUST_IOS_TARGETS"
# shellcheck disable=SC2086 # a list
"$HOME/.cargo/bin/rustup" target add $RUST_IOS_TARGETS
rm -rf "$HOME/.rustup/downloads" "$HOME/.rustup/tmp"

say "sccache $SCCACHE_VERSION, XcodeGen $XCODEGEN_VERSION"
f="$(verified "$SCCACHE_URL" "$SCCACHE_SHA256")"; tar -xzf "$f" -C "$W"
sudo_run install -m 0755 "$W"/sccache-*/sccache /usr/local/bin/sccache
f="$(verified "$XCODEGEN_URL" "$XCODEGEN_SHA256")"; ditto -x -k "$f" "$W/xcodegen"
sudo_run rm -rf /usr/local/lib/xcodegen
sudo_run mkdir -p /usr/local/lib/xcodegen
sudo_run ditto "$W/xcodegen/xcodegen" /usr/local/lib/xcodegen
# A wrapper, not a link: XcodeGen finds its setting presets (share/) relative to
# the path it was started by, and a link in /usr/local/bin has none.
printf '#!/bin/sh\nexec /usr/local/lib/xcodegen/bin/xcodegen "$@"\n' >"$W/xcodegen-wrapper"
sudo_run install -m 0755 "$W/xcodegen-wrapper" /usr/local/bin/xcodegen

say "CocoaPods $COCOAPODS_VERSION"
HOMEBREW_NO_AUTO_UPDATE=1 HOMEBREW_NO_ANALYTICS=1 HOMEBREW_NO_ENV_HINTS=1 /opt/homebrew/bin/brew install cocoapods
got="$(LANG=en_US.UTF-8 /opt/homebrew/bin/pod --version)"
[ "$got" = "$COCOAPODS_VERSION" ] || { echo "CocoaPods is $got, not $COCOAPODS_VERSION: Homebrew's formula moved; update versions.env" >&2; exit 1; }
sudo_run ln -sfn /opt/homebrew/bin/pod /usr/local/bin/pod
# CocoaPods insists on a UTF-8 locale.
grep -q 'LANG=en_US.UTF-8' "$HOME/.zprofile" 2>/dev/null || printf 'export LANG=en_US.UTF-8\n' >>"$HOME/.zprofile"
/opt/homebrew/bin/brew cleanup -s >/dev/null 2>&1 || true
rm -rf "$HOME/Library/Caches/Homebrew"

say "developer mode"
# No debugger or Instruments authorization prompts for the agent's account,
# and no SwiftPM macro or build-plugin trust dialogs (the agent cannot click
# through a trust prompt in a headless build).
sudo_run DevToolsSecurity -enable
sudo_run dseditgroup -o edit -a "$(id -un)" -t user _developer
defaults write com.apple.dt.Xcode IDESkipMacroFingerprintValidation -bool YES
defaults write com.apple.dt.Xcode IDESkipPackagePluginFingerprintValidatation -bool YES
defaults write com.apple.dt.Xcode IDEPackageSupportDisableManifestSandbox -bool YES

say "Simulator device"
runtime="$(xcrun simctl list runtimes -j | python3 -c '
import json, sys
rs = [r for r in json.load(sys.stdin)["runtimes"] if r.get("isAvailable") and r["identifier"].split(".")[-1].startswith("iOS")]
print(rs[-1]["identifier"] if rs else "")')"
[ -n "$runtime" ] || { echo "no iOS simulator runtime is available" >&2; exit 1; }
devtype="$(xcrun simctl list devicetypes -j | python3 -c '
import json, sys
t = [d["identifier"] for d in json.load(sys.stdin)["devicetypes"] if "iPhone" in d["name"] and "Pro" in d["name"] and "Max" not in d["name"]]
print(t[-1] if t else "")')"
[ -n "$devtype" ] || devtype="$(xcrun simctl list devicetypes | sed -n 's/.*(\(com.apple.CoreSimulator.SimDeviceType.iPhone[^)]*\)).*/\1/p' | tail -1)"
xcrun simctl delete "$SIMULATOR_DEVICE_NAME" >/dev/null 2>&1 || true
udid="$(xcrun simctl create "$SIMULATOR_DEVICE_NAME" "$devtype" "$runtime")"
echo "   $SIMULATOR_DEVICE_NAME $udid ($runtime)"
# Boot it once and shut it down: the first boot builds the runtime's caches.
xcrun simctl boot "$udid" || true
xcrun simctl bootstatus "$udid" -b || true
xcrun simctl shutdown "$udid" || true

say "runtimes"
xcrun simctl list runtimes
rm -rf "$HOME/Library/Caches/com.apple.dt.Xcode" "$HOME/Library/Developer/Xcode/DerivedData" "$HOME/Library/Caches/CocoaPods"
rm -rf "$HOME/Library/Developer/CoreSimulator/Caches" 2>/dev/null || true

say "versions"
xcodebuild -version | head -2
"$HOME/.cargo/bin/rustup" target list --installed
/usr/local/bin/sccache --version; /usr/local/bin/xcodegen --version; pod --version 2>/dev/null || /usr/local/bin/pod --version
echo "xcode tier installed"
