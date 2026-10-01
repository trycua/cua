#!/bin/bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# suppress-setup-assistant.sh — stop MiniBuddy from owning a Space's first login.
#
# Runs IN THE GUEST, near the end of the build. Must run after everything that
# can trigger a self-update, and before sanitize.
#
# The problem: `lume create --unattended` answers the Setup Assistant panes that
# exist in the IPSW, macOS then updates itself, and the post-update Setup
# Assistant ("MiniBuddy") is armed to show what is new. Every clone of the image
# then boots into "Update Mac Automatically" -> "Sign In to Your Apple Account",
# which owns the session, so nothing an agent does can start.
#
# The fix is ONE pref, and it must be DELETED, not set to false.
# loginwindow's -[Login1 miniBuddyOption] does not consult
# com.apple.SetupAssistant at all; it asks whether `MiniBuddyLaunch` EXISTS in
# the user's com.apple.loginwindow domain. Existence, not value:
#
#   MiniBuddyLaunch = 1      -> "MiniBuddyLaunch pref is set,
#   MiniBuddyLaunch = 0      ->  setting miniBuddyOption to
#                                kMinibuddyOptionMBPrefSet"   -> MiniBuddy runs
#   key absent               -> "MiniBuddyLaunch pref is NOT set" -> no MiniBuddy
#
# Both of those were observed in the guest's own log on the same image. Writing
# `-bool false` is the trap: it looks like a fix, the log still says "pref is
# set", and Setup Assistant still owns the session.
#
# So every DidSee*/LastSeen* key can be perfect and MiniBuddy still runs. That
# is exactly what was observed before this script existed.
#
# NEVER pkill Setup Assistant to get rid of it. loginwindow reads the abnormal
# exit as an aborted setup and forces a logout ~11 s into boot, taking the Dock,
# WindowServer and all four com.trycua agents with it.
#
# Nothing here names an OS version: the pane state is keyed to `sw_vers` at run
# time, so the script stays correct under a future `--ipsw latest`.
set -uo pipefail

# sudo cannot prompt over SSH; feed the password in (same convention as the
# other build steps).
sudo_run() { printf '%s\n' "${CUA_SUDO_PW:-lume}" | sudo -S -p '' "$@"; }

OSV="$(sw_vers -productVersion)"   # e.g. 26.6.2
BLD="$(sw_vers -buildVersion)"     # e.g. 25G83
UID_GUI=501

echo "== suppress-setup-assistant: macOS $OSV ($BLD)"

# 1. The pref loginwindow actually consults. This is the load-bearing line, and
#    it must be written STRAIGHT TO THE FILE.
#
#    `defaults write` does not work here anyway. While MiniBuddy is running it
#    holds com.apple.loginwindow open in the user's cfprefsd, and a write into
#    that domain is absorbed by the cache and never reaches disk — measured:
#    after `sudo launchctl asuser 501 defaults write com.apple.loginwindow
#    MiniBuddyLaunch -bool false` returned success and `defaults read` answered
#    0, `plutil -p` on the file still showed `MiniBuddyLaunch => true` with
#    `MiniBuddyLaunchCount => 10`. The image then ships re-armed while every
#    check in the build says it is fine.
LW=/Users/lume/Library/Preferences/com.apple.loginwindow.plist
pb() { sudo_run /usr/libexec/PlistBuddy -c "$1" "$2" >/dev/null 2>&1; }
pb "Delete :MiniBuddyLaunch"      "$LW"
pb "Delete :MiniBuddyLaunchCount" "$LW"

# ByHost copies of the same domain shadow the base domain when their UUID
# matches the host. A clone gets a fresh IOPlatformUUID so it normally falls
# through to the base domain, but the golden accumulates one stale file per
# identity rotation and each carries MiniBuddyLaunch=1. They hold nothing else.
sudo_run rm -f /Users/lume/Library/Preferences/ByHost/com.apple.loginwindow.*.plist

# 2. Pre-answer the panes themselves, in BOTH the user domain and the system
#    domain. The system domain is what a fresh login reads, and on a freshly
#    built image it lags the running OS (observed: 25F84 / 26.5.2 under a
#    26.6.2 system) — a LastSeen value behind the OS is what arms the
#    new-feature panes in the first place. Keyed to sw_vers, never to a literal.
for key in DidSeeCloudSetup DidSeeSiriSetup DidSeePrivacy DidSeeAppearanceSetup \
           DidSeeTrueToneSetup DidSeeAccessibility DidSeeActivationLock \
           DidSeeAppStore DidSeeApplePaySetup DidSeeExpressSettings \
           DidSeeLockdownMode DidSeeScreenTime DidSeeSoftwareUpdateSetup \
           DidSeeSyncSetup DidSeeSyncSetup2 DidSeeTermsOfAddress \
           DidSeeTouchIDSetup DidSeeiCloudLoginForStorageServices \
           SkipFirstLoginOptimization SkipExpressSettingsUpdating; do
  sudo_run launchctl asuser "$UID_GUI" defaults write com.apple.SetupAssistant "$key" -bool true
  sudo_run defaults write /Library/Preferences/com.apple.SetupAssistant "$key" -bool true
done
for d in "launchctl asuser $UID_GUI defaults write com.apple.SetupAssistant" \
         "defaults write /Library/Preferences/com.apple.SetupAssistant"; do
  sudo_run $d LastSeenBuddyBuildVersion -string "$BLD"
  sudo_run $d LastSeenCloudProductVersion -string "$OSV"
  sudo_run $d MiniBuddyShouldLaunchToResumeSetup -bool false
  sudo_run $d MiniBuddyLaunchReason -int 0
done

# 3. Do not let the OS update itself inside a Space and re-arm MiniBuddy.
#    Without this the suppression is only true until the next minor release.
for key in AutomaticCheckEnabled AutomaticDownload AutomaticallyInstallMacOSUpdates \
           CriticalUpdateInstall ConfigDataInstall; do
  sudo_run defaults write /Library/Preferences/com.apple.SoftwareUpdate "$key" -bool false
done
sudo_run defaults write /Library/Preferences/com.apple.commerce AutoUpdate -bool false
sudo_run softwareupdate --schedule off >/dev/null 2>&1 || true

# 4. Setup is done, system-wide.
sudo_run touch /var/db/.AppleSetupDone

# Drop the user's stale in-memory preference domains, LAST, so nothing can flush
# an old copy back over the files and so the verification below reads exactly
# what will ship. Everything above has already synchronized to disk.
sudo_run launchctl asuser "$UID_GUI" killall cfprefsd >/dev/null 2>&1 || true
sleep 2

# 5. Verify ON DISK, never through `defaults read`.
#
# This is the trap that made the first fix look like it worked and ship broken.
# While MiniBuddy is running it holds com.apple.loginwindow open in the user's
# cfprefsd, and `defaults read` answers out of that cache. Writing 0 and reading
# 0 back proves nothing: the file still said `MiniBuddyLaunch => true` with the
# launch counter still incrementing (9 -> 10 across the clone's boot), so the
# published image shipped re-armed while every `defaults read` said 0.
#
# So: read the plist, and refuse to certify the image while Setup Assistant is
# still alive. build-golden.sh reboots the guest and runs this script a second
# time; that second pass is the one that can pass, because MiniBuddy no longer
# launches and nothing is holding the domain.
PL=/Users/lume/Library/Preferences/com.apple.loginwindow.plist
echo "-- verify (on disk)"
sudo_run plutil -p "$PL" 2>&1 | sed 's/^/   /'

disk_mb="$(sudo_run plutil -extract MiniBuddyLaunch raw -o - "$PL" 2>/dev/null)"
byhost="$(ls -1 /Users/lume/Library/Preferences/ByHost/com.apple.loginwindow.*.plist 2>/dev/null | wc -l | tr -d ' ')"
running="$(pgrep -f '/Setup Assistant.app/Contents/MacOS/Setup Assistant' | wc -l | tr -d ' ')"

if [ -n "$disk_mb" ]; then
  echo "FAILED: MiniBuddyLaunch still EXISTS on disk (value '$disk_mb')." >&2
  echo "  loginwindow keys off existence, not value — the key must be gone." >&2
  exit 1
fi
if [ "$byhost" != "0" ]; then
  echo "FAILED: $byhost ByHost com.apple.loginwindow plist(s) still present" >&2; exit 1
fi
if [ "$running" != "0" ]; then
  echo "NOT YET CERTIFIED: Setup Assistant is still running, so its cfprefsd copy" >&2
  echo "  can still overwrite this on shutdown. Reboot the guest and re-run." >&2
  exit 2
fi
echo "ok: MiniBuddy disarmed for macOS $OSV ($BLD), verified on disk, no Setup Assistant running"
