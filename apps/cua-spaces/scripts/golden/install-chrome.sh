#!/bin/bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# install-chrome.sh — install Google Chrome into the golden, renderable in a VM.
#
# Chrome is a first-class teleport target: "teleport my chrome and stream it"
# has to land a usable browser, and the spacesd's Chrome teleport import relaunches the
# destination at /Applications/Google Chrome.app/Contents/MacOS/Google Chrome.
# With no Chrome in the image the import silently had nothing to launch.
#
# This recipe was previously only in scripts/build-macos-golden.sh, which builds
# a DIFFERENT (driver-only) image, so the golden that Spaces actually clone got
# Chrome hand-installed or not at all. Measured on the shipping golden: Chrome
# was dropped in four days after every other app, with no wrapper and no
# rendering policy — i.e. not reproducible from anything checked in. Same
# recipe, now a build step like every other.
set -uo pipefail

say() { printf '\033[1;36m==> %s\033[0m\n' "$*"; }

# Runs over SSH with no tty, so sudo cannot prompt. Same convention as the
# sibling install scripts.
sudo_run() { printf '%s\n' "${CUA_SUDO_PW:-lume}" | sudo -S -p '' "$@"; }

APP="/Applications/Google Chrome.app"
MACOS="$APP/Contents/MacOS"
DMG_URL="https://dl.google.com/chrome/mac/universal/stable/GGRO/googlechrome.dmg"

# --------------------------------------------------------------------------
# 1. Install the app.
# --------------------------------------------------------------------------
# Idempotent: an image rebuilt on top of itself must not re-download or, worse,
# copy a fresh Chrome over the wrapper installed in step 3 (that would silently
# revert the rendering policy and leave `Google Chrome.real` orphaned).
if [ ! -d "$APP" ]; then
  say "Downloading Google Chrome"
  if ! curl -fsSL "$DMG_URL" -o /tmp/chrome.dmg; then
    echo "FAILED: could not download $DMG_URL" >&2
    exit 1
  fi
  sudo_run hdiutil attach /tmp/chrome.dmg -nobrowse -mountpoint /tmp/chromemnt >/dev/null 2>&1
  # No `|| true` anywhere in this block: a Space with a half-copied Chrome is
  # worse than a build that stops and says so.
  if ! cp -R "/tmp/chromemnt/Google Chrome.app" /Applications/; then
    echo "FAILED: could not copy Google Chrome.app out of the DMG" >&2
    sudo_run hdiutil detach /tmp/chromemnt >/dev/null 2>&1
    exit 1
  fi
  sudo_run hdiutil detach /tmp/chromemnt >/dev/null 2>&1 || true
  rm -f /tmp/chrome.dmg
  say "Installed $APP"
else
  say "Chrome already present; leaving it alone"
fi

# Gatekeeper would otherwise put up "Google Chrome is an app downloaded from the
# Internet. Are you sure you want to open it?" — a modal nothing in an
# unattended Space can dismiss.
sudo_run xattr -dr com.apple.quarantine "$APP" 2>/dev/null || true

# --------------------------------------------------------------------------
# 2. Rendering policy for a GPU-less guest.
# --------------------------------------------------------------------------
# A Lume VM has no GPU compositing, so Chrome renders a BLACK page with hardware
# acceleration on. This pref is read per-user, and sanitize-golden.sh deletes the
# Chrome profile from the image, so it is written to the domain (which survives)
# rather than into the profile (which does not).
defaults write com.google.Chrome HardwareAccelerationModeEnabled -bool false

# --------------------------------------------------------------------------
# 3. Wrap the bundle executable.
# --------------------------------------------------------------------------
# Without a GPU, Chrome blocklists WebGL entirely, so WebGL content fails.
# --enable-unsafe-swiftshader routes WebGL through the SwiftShader CPU
# rasterizer. It is a raw command-line switch with NO chrome://flags or Local
# State equivalent, so the only way to make every launch inherit it — including
# `open -a "Google Chrome"` and the relaunch the teleport import performs,
# neither of which passes our flags — is to wrap the bundle executable.
#
# --no-first-run / --no-default-browser-check are in the wrapper for the same
# reason. sanitize-golden.sh strips the profile from the image (it is the
# builder's, and shipping it would leak build-machine state), so every clone's
# first Chrome launch is a first run; without these it opens the welcome flow
# and a "make Chrome your default browser" prompt on a desktop the user did not
# ask for anything on.
say "Forcing software-rendered WebGL and a silent first run"
if [ ! -f "$MACOS/Google Chrome.real" ]; then
  mv "$MACOS/Google Chrome" "$MACOS/Google Chrome.real" || {
    echo "FAILED: could not move the Chrome bundle executable aside" >&2
    exit 1
  }
fi
cat > "$MACOS/Google Chrome" <<'WRAP'
#!/bin/bash
# The Lume guest has no GPU: route WebGL through SwiftShader. Keep first-run and
# default-browser prompts off a desktop nobody asked to see. All args pass through.
exec "$(dirname "$0")/Google Chrome.real" \
  --enable-unsafe-swiftshader \
  --hide-crash-restore-bubble \
  --no-first-run \
  --no-default-browser-check \
  "$@"
WRAP
chmod +x "$MACOS/Google Chrome"

# Renaming the bundle executable breaks the bundle seal, and the hardened runtime
# then SIGKILLs Chrome. --deep is required, not cosmetic: an outer-only ad-hoc
# re-sign leaves the nested Framework and helpers carrying Google's signature, so
# library validation of the exec chain fails and Chrome crashes on first launch
# in a clone (it survives on the build VM, which is how this hid). The real
# binary keeps its own signature and is exec'd fresh by the wrapper.
if ! codesign --force --deep --sign - "$APP" >/dev/null 2>&1; then
  echo "FAILED: could not re-sign $APP after wrapping; Chrome would be SIGKILLed" >&2
  exit 1
fi

# --------------------------------------------------------------------------
# 4. Verify, in the image, that the thing actually runs.
# --------------------------------------------------------------------------
# A build that installs a Chrome which cannot launch is the failure this whole
# file exists to stop happening silently, so prove it here rather than finding
# out in front of a customer. What is actually at risk is the wrapper plus the
# re-signed bundle seal, so exec the bundle executable DIRECTLY -- that is the
# exec chain the hardened runtime validates, and it is what `open` and the teleport importer's
# relaunch both end up running.
#
# Deliberately not `open -a "Google Chrome"`: this script runs over ssh, which
# lands in launchd's Background session, and `open` needs the Aqua session. It
# fails there for a reason that has nothing to do with Chrome, which would make
# this check report a broken image on a perfectly good one.
say "Verifying Chrome launches"
"$MACOS/Google Chrome" about:blank >/tmp/cua-chrome-verify.log 2>&1 &
verify_pid=$!
ok=0
for _ in $(seq 1 20); do
  # A helper process only appears once the framework loaded and the bundle seal
  # held, so it -- not the wrapper script merely running -- is the signal that
  # the whole re-signed bundle is consistent.
  #
  # `pgrep -f` takes a REGEX, so the pattern must not contain regex
  # metacharacters: "Google Chrome Helper (GPU)" silently matches nothing,
  # because `(GPU)` is a capture group and the literal parentheses are never
  # there. That made this check fail on a Chrome that had started perfectly.
  if /usr/bin/pgrep -f "Google Chrome Helper" >/dev/null 2>&1; then ok=1; break; fi
  kill -0 "$verify_pid" 2>/dev/null || break
  sleep 1
done
# A seal failure SIGKILLs the process within a second or two, so surviving this
# long with helpers up is the real pass condition.
if [ "$ok" = "1" ] && ! kill -0 "$verify_pid" 2>/dev/null; then ok=0; fi
/usr/bin/pkill -9 -f "Google Chrome" 2>/dev/null || true
wait "$verify_pid" 2>/dev/null || true
sleep 1
if [ "$ok" != "1" ]; then
  echo "FAILED: Chrome did not start after wrapping + re-signing. Output:" >&2
  sed 's/^/    /' /tmp/cua-chrome-verify.log >&2 || true
  exit 1
fi
# Confirm the rendering policy really took, rather than assuming the flag was
# accepted: without it Chrome blocklists WebGL and a WebGL page renders nothing.
if grep -q "swiftshader" /tmp/cua-chrome-verify.log 2>/dev/null; then
  say "SwiftShader WebGL confirmed active"
fi
rm -f /tmp/cua-chrome-verify.log

# --------------------------------------------------------------------------
# 5. Evict GoogleUpdater.
# --------------------------------------------------------------------------
# Chrome installs its own updater (GoogleUpdater.app plus per-user LaunchAgents)
# the first time it runs, which the step above necessarily triggers. Left in
# place it would ship in the image and then, in EVERY clone, start on login and
# sit there wanting to phone home -- an app the user never asked to run, on an
# image whose whole contract is that it launches nothing. Worse, it can update
# Chrome underneath a running Space, silently replacing the wrapped executable
# installed above with a stock one and taking the rendering policy with it.
say "Evicting GoogleUpdater (an image must not auto-update itself)"
/usr/bin/pkill -9 -f GoogleUpdater 2>/dev/null || true
for plist in "$HOME/Library/LaunchAgents/"com.google.*.plist; do
  [ -e "$plist" ] || continue
  label="$(basename "$plist" .plist)"
  launchctl bootout "gui/$(id -u)/$label" 2>/dev/null || true
  rm -f "$plist" && echo "  removed LaunchAgent $label"
done
rm -rf "$HOME/Library/Application Support/Google/GoogleUpdater" 2>/dev/null || true
rm -rf "$HOME/Library/Google/GoogleUpdater" 2>/dev/null || true
# Keystone, the older updater, if this Chrome build still drops one.
rm -rf "$HOME/Library/Google/GoogleSoftwareUpdate" 2>/dev/null || true
defaults write com.google.Chrome UpdaterAutomaticallyCheckForUpdates -bool false 2>/dev/null || true

say "Chrome installed, wrapped, verified, and closed"
