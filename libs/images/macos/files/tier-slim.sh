#!/bin/bash
# The slim tier's additions to the Lume base (beyond cua-spacesd, which
# install-guest.sh installs): Google Chrome, so cua-driver's browser tools
# (CDP) work on every tier. Runs IN the guest as the desktop user.
#
#   tier-slim.sh STAGE_DIR
#
# STAGE_DIR holds versions.env and cache/<the pinned downloads> (build.sh
# fetched and checked them on the host); each file is checked again here.
set -euo pipefail
STAGE="${1:?staging directory}"
# shellcheck source=/dev/null # STAGE/versions.env, a copy of ../versions.env
. "$STAGE/versions.env"
# The password on a here-string, not a pipe: with sudo's credential cache
# warm, sudo never reads it, and a printf pipe then dies of SIGPIPE, which
# pipefail turns into a failed step.
sudo_run() { sudo -S -p '' "$@" <<<"${CUA_SUDO_PW:-lume}"; }
say() { echo "==> $*"; }
verified() {  # FILE SHA256
    [ "$(shasum -a 256 "$1" | cut -d' ' -f1)" = "$2" ] || { echo "checksum mismatch: $1" >&2; exit 1; }
    echo "$1"
}

say "Google Chrome $CHROME_VERSION"
dmg="$(verified "$STAGE/cache/$(basename "$CHROME_URL")" "$CHROME_SHA256")"
mnt="$(mktemp -d /tmp/cua-build-chrome.XXXXXX)"
hdiutil attach -nobrowse -readonly -noverify -mountpoint "$mnt" "$dmg" >/dev/null
trap 'hdiutil detach "$mnt" -force >/dev/null 2>&1 || true; rm -rf "$mnt"' EXIT
sudo_run rm -rf "/Applications/Google Chrome.app"
sudo_run ditto "$mnt/Google Chrome.app" "/Applications/Google Chrome.app"
sudo_run xattr -dr com.apple.quarantine "/Applications/Google Chrome.app" 2>/dev/null || true
# Google's signature (team EQHXZ8M8AV), on top of the pinned checksum.
codesign --verify --deep "/Applications/Google Chrome.app"
codesign -dv "/Applications/Google Chrome.app" 2>&1 | grep -qx 'TeamIdentifier=EQHXZ8M8AV' ||
    { echo "Google Chrome.app is not signed by Google" >&2; exit 1; }
got="$(defaults read "/Applications/Google Chrome.app/Contents/Info" CFBundleShortVersionString)"
[ "$got" = "$CHROME_VERSION" ] || { echo "Chrome is $got, not $CHROME_VERSION" >&2; exit 1; }

# The image stays at the pinned version (no background updater), and a
# fresh profile starts without first-run, sign-in or default-browser prompts.
# Managed preferences in /Library/Managed Preferences are Chrome policies.
say "Chrome policies"
POL="/Library/Managed Preferences/com.google.Chrome.plist"
# cfprefsd does not take `defaults write` there; write the file itself.
sudo_run mkdir -p "/Library/Managed Preferences"
sudo_run plutil -create xml1 "$POL"
for kv in "BrowserSignin -integer 0" "SyncDisabled -bool YES" "PasswordManagerEnabled -bool NO" \
    "PromotionsEnabled -bool NO" "DefaultBrowserSettingEnabled -bool NO" \
    "MetricsReportingEnabled -bool NO" "PrivacySandboxPromptEnabled -bool NO" \
    "TranslateEnabled -bool NO"; do
    # shellcheck disable=SC2086 # key, type and value are separate words
    sudo_run plutil -insert $kv "$POL"
done
sudo_run chmod 0644 "$POL"
plutil -lint "$POL"
defaults write com.google.Keystone.Agent checkInterval 0
echo "slim tier installed"
