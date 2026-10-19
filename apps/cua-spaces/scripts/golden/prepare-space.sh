#!/bin/bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# prepare-space.sh — per-boot preparation of a Space. Runs in the aqua (GUI)
# session via the com.trycua.prepare-space LaunchAgent (RunAtLoad).
#
# This does only GENERIC work. It installs NO credentials: every identity
# (Unity account, UVCS, Claude, browser profiles) arrives by teleport at
# runtime. An earlier version of this script baked the builder's Unity OAuth
# token, Safe Storage key and account id into the image and reinstalled them on
# every boot — publishing that image would publish those credentials.
#
# Everything here has to happen PER BOOT rather than once at build time,
# because the applications themselves rewrite it. Unity Hub, for instance,
# overwrites every onboarding marker the build wrote the first time it runs —
# which it does during the build — so the golden ships the *reset* values and
# the wizard comes back in every Space unless they are rewritten at boot.
exec >>"$HOME/.cua/prepare-space.log" 2>&1
echo "=== $(date) prepare start ==="

sudo_run() { printf '%s\n' "${CUA_SUDO_PW:-lume}" | sudo -S -p '' "$@"; }

# --------------------------------------------------------------------------
# 0. Clear the "blocked by a credential panel" marker from the LAST boot.
# --------------------------------------------------------------------------
# permission-approver.py drops ~/.cua/SPACE-BLOCKED-credential-panel.txt when it
# meets a keychain panel it cannot answer, so an agent can tell a stalled Space
# from a slow one without parsing a log. Nothing ever removed it.
#
# So it outlives the condition. Measured on cua-space-e3c1b54907: the marker
# said "THIS SPACE IS BLOCKED ... Unity Hub wants to access key 'unity' ...
# Do not keep waiting", with an mtime five hours before the running boot, on a
# Space where the panel was long gone (no SecurityAgent process, login keychain
# healthy and taking the account password). It also ships inside the golden if
# the build ever tripped one, which makes EVERY clone claim to be blocked.
#
# A marker that says "blocked" on a healthy Space is worse than no marker: it is
# the silent-failure rule pointed the other way, telling an agent to give up on
# something that works. It describes the CURRENT boot or it does not exist.
rm -f "$HOME/.cua/SPACE-BLOCKED-credential-panel.txt"

# --------------------------------------------------------------------------
# 0. Permission ledgers — FIRST, before anything slow.
# --------------------------------------------------------------------------
# This used to be step 5, after the ~10-20 s keychain section and the Dock wait.
# That is too late. In the failing run the alert
#   "<capture daemon> is requesting to bypass the system private window picker"
# fired 83 s after boot, on the capture daemon's FIRST capture (today
# cua-spacesd's, then rcdpd's), which happens as soon as a
# viewer attaches — routinely before this script's keychain work finishes. The
# ledger that schedules that alert (see seed-tcc.sh) must already be on disk by
# then, so it is refreshed here, at the top, and it needs no GUI session to do
# it: it is a plist write plus sqlite timestamps.
if [ -x "$HOME/.cua/seed-tcc.sh" ]; then
  "$HOME/.cua/seed-tcc.sh" --refresh 2>&1 | sed 's/^/  tcc: /'
fi

# --------------------------------------------------------------------------
# 0a. MiniBuddy — the post-update Setup Assistant — must not be on screen.
# --------------------------------------------------------------------------
# A Space built from an IPSW-installed base VM came up showing
#   "Update Mac Automatically — Only Download Automatically / Continue"
# over the desktop, on EVERY clone. This is not the first-run Setup Assistant
# (/var/db/.AppleSetupDone exists); it is MiniBuddy, which `loginwindow` runs
# after an OS version change to show whatever panes are new. It decides that
# from two keys in com.apple.SetupAssistant:
#
#   LastSeenBuddyBuildVersion  = 25F84    <- what the image was set up on
#   LastSeenCloudProductVersion = 26.5.2
#
# against the running system's build. `lume create --unattended` pre-answers
# the panes that exist at install time and then the OS ships a newer build, so
# the two drift apart and MiniBuddy relaunches.
#
# It is worse than cosmetic: while Setup Assistant owns the session,
# `screencapture` fails outright with "could not create image from display",
# so the Space looks like the no-display failure mode this doc warns about
# while actually having a perfectly good display.
#
# Fix, per boot, before anything tries to capture: claim every pane as seen,
# pin the two version keys to the RUNNING system, and kill any MiniBuddy that
# is already up.
suppress_minibuddy() {
  local build ver
  build="$(/usr/bin/sw_vers -buildVersion)"
  ver="$(/usr/bin/sw_vers -productVersion)"
  for k in DidSeeAccessibility DidSeeActivationLock DidSeeAppStore \
           DidSeeAppearanceSetup DidSeeApplePaySetup DidSeeCloudSetup \
           DidSeeLockdownMode DidSeePrivacy DidSeeScreenTime DidSeeSiriSetup \
           DidSeeSyncSetup DidSeeSyncSetup2 DidSeeTermsOfAddress \
           DidSeeTouchIDSetup DidSeeTrueToneSetup \
           DidSeeiCloudLoginForStorageServices; do
    /usr/bin/defaults write com.apple.SetupAssistant "$k" -bool true
  done
  /usr/bin/defaults write com.apple.SetupAssistant LastSeenBuddyBuildVersion -string "$build"
  /usr/bin/defaults write com.apple.SetupAssistant LastSeenCloudProductVersion -string "$ver"
  /usr/bin/defaults write com.apple.SetupAssistant MiniBuddyShouldLaunchToResumeSetup -bool false
  /usr/bin/defaults write com.apple.SetupAssistant SkipFirstLoginOptimization -bool true
  # The "Update Mac Automatically" pane specifically. Pinning the two version
  # keys above is NOT enough on its own — measured: a clone whose
  # LastSeenBuddyBuildVersion already matched the running build still showed
  # it. This is the Express Settings updating pane and it has its own key.
  /usr/bin/defaults write com.apple.SetupAssistant SkipExpressSettingsUpdating -bool true
  /usr/bin/defaults write com.apple.SetupAssistant MiniBuddyLaunchReason -int 0
  /usr/bin/defaults write com.apple.SetupAssistant DidSeeSoftwareUpdateSetup -bool true
  /usr/bin/defaults write com.apple.SetupAssistant DidSeeExpressSettings -bool true
  # Flush cfprefsd so the file on disk — which is what the NEXT login reads,
  # and what a clone of this image inherits — actually carries the values.
  /usr/bin/defaults read com.apple.SetupAssistant >/dev/null 2>&1
  echo "minibuddy suppressed (build $build / $ver)"
  # DO NOT KILL A RUNNING SETUP ASSISTANT. An earlier revision ran
  #   pkill -f '/System/Library/CoreServices/Setup Assistant.app'
  # which also matches MiniBuddy's helpers (mbuseragent, mbusertrampoline,
  # mbsystemadministration, all under that bundle's Resources/). loginwindow
  # reads their abnormal exit as an aborted setup and FORCES A LOGOUT:
  #   WindowServer: loginwindow is unregistering special key forceLogout
  # eleven seconds into boot, taking the whole Aqua session with it — Dock,
  # WindowServer, all four com.trycua agents, Unity Hub and Blender. The Space
  # then answers SSH with nothing running and no session at all.
  #
  # The keys above are the fix, and they work by preventing the NEXT launch.
  # If MiniBuddy is already on screen, this boot is lost to it; the value here
  # is that the image is written correctly so no clone ever sees it again.
  if /usr/bin/pgrep -qf '/System/Library/CoreServices/Setup Assistant.app'; then
    echo "WARNING: Setup Assistant is already running on this boot; screen" \
         "capture will fail until it exits. Leaving it alone (killing it" \
         "forces a logout)."
  fi
}
suppress_minibuddy

# --------------------------------------------------------------------------
# 0b. Wait for the aqua session to be fully up.
# --------------------------------------------------------------------------
# Everything below has to run in the SAME security session as the apps. In
# particular, a keychain read outside the GUI session fails as "not signed in"
# rather than working, so the keychain check below would report a false failure
# if it ran too early. Waiting for the Dock is the cheap, reliable signal that
# the session is real.
#
# Note for anyone tempted to create or swap a keychain here: don't. A keychain
# swapped in under a session that has already opened the old one by identity
# reads as a different, locked keychain, and the first app to touch it raises
#   "Unity Hub wants to use the 'login' keychain. Please enter the keychain
#    password."
# — a SecurityAgent dialog drawn in a separate secure session, so it does not
# appear in the accessibility tree and macOS ignores synthetic input to it.
# Nothing in an unattended Space can clear it. Keychain identity is settled at
# BUILD time (sanitize-golden.sh), which is why nothing below has to be ordered.
for _ in $(seq 1 90); do
  /usr/bin/pgrep -qx Dock && break
  sleep 1
done
echo "aqua session up after ${SECONDS}s (Dock pid $(/usr/bin/pgrep -x Dock | head -1))"

# --------------------------------------------------------------------------
# 0c. Restart cua-driver AFTER the TCC seed, and prove its gate opened.
# --------------------------------------------------------------------------
# This is the fix for the boot race that made roughly half of all clones dead on
# arrival: every screenshot request answering, for the
# whole life of the Space,
#   tool='start_session', message='permissions_pending: macOS Accessibility or
#   Screen Recording permission is still pending', error_code='75'
# while `sqlite3 /Library/Application Support/com.apple.TCC/TCC.db` showed
#   kTCCServiceAccessibility|com.trycua.driver.local|2
#   kTCCServiceScreenCapture|com.trycua.driver.local|2
# Reproduced here on the untouched golden: 2 dead clones in 6 sequential boots.
# On a captured failing clone, asked as uid 501 inside the aqua session,
#   cua-driver-local permissions status
#     Accessibility:    ❌ not granted
#     Screen Recording: ❌ not granted
#     Source: driver-daemon
# with both rows sitting at auth_value=2 in the system TCC.db at the same moment.
#
# THE RACE. com.trycua.cua-driver-local is its own RunAtLoad + KeepAlive
# LaunchAgent (install-agent-stack.sh), so launchd starts it at login IN
# PARALLEL with com.trycua.prepare-space. Whoever wins decides whether the
# driver ever sees its grants. Note that nothing in this script previously
# started the driver at all — an earlier investigation reported that
# prepare-space "starts the driver LaunchAgent and only then seeds TCC"; that is
# not what happens, and the ordering is not this script's to lose. seed-tcc.sh
# genuinely does run first here, at line ~32.
#
# WHERE THE STALE STATE LIVES, and it is not where it looks. cua-driver's gate
# is not the thing caching a denial: it re-probes once a second by spawning a
# fresh short-lived copy of the signed executable, precisely so the long-lived
# daemon never caches a negative preflight
# (platform-macos/src/permissions/gate.rs, fresh_status_with_request). The stale
# cache is in the SYSTEM tccd — the daemon that serves Accessibility and Screen
# Recording — which seed-tcc.sh's per-boot refresh rewrites with sqlite3 behind
# its back and, until now, never restarted. seed-tcc.sh now kickstarts both
# tccds on every boot; see the long note on restart_tcc_clients there.
#
# WHY THE DRIVER STILL HAS TO BE RESTARTED HERE. Fixing the tccd is necessary
# and not sufficient, because the gate is a ONE-SHOT. cua-driver/src/main.rs
# sets PERMISSION_GATE_PENDING=true before startup and clears it in exactly one
# place — if the single run_if_needed() call on the main thread returns Ok. On
# its ten-minute deadline it bails, and the flag then stays true for the LIFE OF
# THE PROCESS: every tool call returns error 75 forever even after the grants
# become readable seconds later. There is no re-arm and no periodic re-check.
# So a driver that lost the race cannot recover on its own, and the only cure is
# a fresh driver process started strictly after the tccd restart. That is what
# this does, and it is why every recovery tried on the original failing clone
# failed: restarting the DRIVER alone leaves the stale system-tccd cache in
# place, and re-running prepare-space restarted only the USER tccd.
#
# Deliberately a restart of the existing agent rather than making the agent
# non-RunAtLoad. The plist carries KeepAlive=true, under which launchd starts
# the job whether or not RunAtLoad is set, so removing RunAtLoad would not
# actually delay anything — and a driver that starts at login and is replaced
# ~12 s later is strictly better than one that cannot start until this script
# reaches it. This runs immediately after the Dock wait, before the ~10-20 s
# keychain section, so the window is short and bounded.
#
# THE CHECK IS THE DAEMON'S OWN ANSWER, NOT ITS LOG. An earlier draft of this
# looked for "[cua-driver] still waiting on" in the driver's stdout log. That is
# unsound in both directions and a failing clone proved it: its out.log was
# byte-for-byte the golden's baked-in copy, mtime 21:05, untouched by the boot —
# while `permissions status` on the running daemon said "❌ not granted" for
# both. The gate had taken its fast path at startup, when the grants still read
# green, printed nothing, and cleared the flag; the system tccd only started
# lying afterwards. A log that says nothing means "the gate did not complain",
# which is not the same as "the driver can see".
#
# `cua-driver-local permissions status` asks the RUNNING DAEMON over its unix
# socket and reports what the daemon itself gets back, tagged `Source:
# driver-daemon`. That is the only answer that matters, and it is the same
# question the desktop services will ask a moment later.
#
# It must be asked as uid 501 from inside the aqua session — which is exactly
# where this script runs, so a bare call is correct. Do NOT be tempted to check
# it over ssh with `sudo launchctl asuser 501 ...`: that connects as uid 0, the
# daemon answers `reject Unix peer uid 0 for runtime owned by uid 501`, and the
# CLI then prints "No CuaDriverLocal daemon is running under the driver's own
# identity". That message is a measurement artifact of asking as root, not
# evidence of a broken daemon, and it is what an earlier investigation of the
# failing clone mistook for the driver having lost its identity.
DRIVER_CLI="/Applications/CuaDriverLocal.app/Contents/MacOS/cua-driver-local"
driver_grants_visible() {
  local out
  out="$("$DRIVER_CLI" permissions status 2>/dev/null)" || return 1
  printf '%s' "$out" | /usr/bin/grep -q "Source: driver-daemon" || return 1
  printf '%s' "$out" | /usr/bin/grep -q "^Accessibility:.*granted"    || return 1
  printf '%s' "$out" | /usr/bin/grep -q "^Screen Recording:.*granted" || return 1
  printf '%s' "$out" | /usr/bin/grep -qE "^(Accessibility|Screen Recording):.*(not granted|unknown)" \
    && return 1
  return 0
}
ensure_driver_permissions() {
  local label=com.trycua.cua-driver-local
  local attempt i
  # Truncate the driver's stdout log once per boot, HERE rather than only in the
  # sanitizer. StandardOutPath appends, so the golden ships whatever the build
  # left in it and every clone inherits it — measured: 23031 bytes, mtime frozen
  # at the build's 21:05, ending in 150 consecutive "[cua-driver] still waiting
  # on: Accessibility, Screen Recording" lines, present and identical on a
  # healthy clone and a dead one alike. It cost two investigations, and the
  # sanitizer's truncation did not survive into clones, so do it per boot where
  # it cannot be missed. Truncating from outside the daemon is safe: the entry
  # below immediately restarts it, so the process holding the old file offset is
  # replaced rather than left writing into a hole.
  : > "$HOME/.cua-server/$label.out.log" 2>/dev/null
  for attempt in 1 2 3; do
    launchctl kickstart -k "gui/$(id -u)/$label" >/dev/null 2>&1
    # The daemon binds its socket in about a second, but give it room under the
    # load of a fresh login (load average is routinely above 4 here).
    for i in $(seq 1 20); do
      sleep 1
      driver_grants_visible && break
    done
    if driver_grants_visible; then
      echo "cua-driver restarted after the TCC seed; Accessibility and Screen Recording visible to the daemon (attempt $attempt)"
      # cua-spacesd needs the same two grants and has the same exposure;
      # restart it after the seed.
      launchctl kickstart -k "gui/$(id -u)/com.trycua.spacesd" >/dev/null 2>&1 \
        || launchctl kickstart -k "gui/$(id -u)/com.trycua.guestd" >/dev/null 2>&1 \
        || launchctl kickstart -k "gui/$(id -u)/com.trycua.env_driver" >/dev/null 2>&1  # older goldens
      return 0
    fi
    echo "cua-driver cannot see its grants after restart $attempt — restarting the system tccd and retrying"
    sudo_run launchctl kickstart -k system/com.apple.tccd.system >/dev/null 2>&1
    sleep 4
  done
  echo "WARNING: cua-driver still cannot see Accessibility/Screen Recording after 3" \
       "restarts. Desktop tool calls will fail. Daemon says:"
  "$DRIVER_CLI" permissions status 2>&1 | sed 's/^/  /'
  echo "  TCC rows on disk:"
  sudo_run /usr/bin/sqlite3 "/Library/Application Support/com.apple.TCC/TCC.db" \
    "select service,client,auth_value from access where client like '%trycua%';" \
    2>/dev/null | sed 's/^/    /'
}
ensure_driver_permissions

# --------------------------------------------------------------------------
# 1. The login keychain, verified openable with the account password.
# --------------------------------------------------------------------------
# cua-spacesd's teleport import (TeleportService) installs each teleported
# secret and then runs
#   security set-generic-password-partition-list -S teamid:<id>,apple: -k <pw>
# to put the destination app's code-signing team into the item's partition
# list. Without that entry macOS refuses the app a silent read and raises
#   "Unity Hub wants to access key 'unity' in your keychain."
# That call takes the KEYCHAIN PASSWORD, so the keychain the secret lands in
# must have a password we know.
#
# That keychain is the LOGIN keychain, and its password is the account
# password. The golden build re-keys it (see sanitize-golden.sh) precisely so
# this is true, because it is the only arrangement that needs no special
# ordering: the login keychain is already the default and already first on the
# search list on any Mac, so a secret written by any code path — a hand-run
# prepare-space.sh, a plain provision, a bare teleport import — lands somewhere
# readable. It also satisfies Unity Hub, whose @unity/hub-keyring resolves the
# DEFAULT keychain only and never walks the search list.
#
# Nothing here creates or switches keychains. This block only verifies the
# invariant the image is built to guarantee, and says so loudly if it is
# broken, because a broken one shows up as a dialog nobody inside a Space can
# answer.
LOGIN="$HOME/Library/Keychains/login.keychain-db"
KEYCHAIN_PW="lume"

/usr/bin/security unlock-keychain -p "$KEYCHAIN_PW" "$LOGIN" 2>/dev/null
/usr/bin/security set-keychain-settings "$LOGIN"   # no auto-lock timeout

# The honest password test is the operation the importer actually needs: an ACL write,
# which takes -k <password> and fails loudly on a wrong one. `unlock-keychain`
# proves nothing here — unlocking an already-unlocked keychain succeeds without
# ever looking at the password.
if /usr/bin/security add-generic-password -U -s cua-keychain-probe -a probe -w x -A "$LOGIN" 2>/dev/null \
   && /usr/bin/security set-generic-password-partition-list \
        -S "teamid:0000000000,apple:,apple-tool:" -k "$KEYCHAIN_PW" \
        -s cua-keychain-probe -a probe "$LOGIN" >/dev/null 2>&1; then
  echo "login keychain ready: account password authorizes ACL writes"
else
  echo "WARNING: the login keychain does not take the account password. The
  first teleport will raise an unanswerable SecurityAgent dialog. The golden
  image's login-keychain re-key (sanitize-golden.sh) did not take effect."
fi
/usr/bin/security delete-generic-password -s cua-keychain-probe -a probe "$LOGIN" >/dev/null 2>&1
# `security` subcommands that take -k unlock the keychain for the call and
# re-lock it on the way out, so the probe above leaves it locked. Re-open it and
# turn auto-lock off: a locked keychain is the "wants to use the 'login'
# keychain" dialog.
/usr/bin/security unlock-keychain -p "$KEYCHAIN_PW" "$LOGIN" 2>/dev/null
/usr/bin/security set-keychain-settings "$LOGIN"

# --------------------------------------------------------------------------
# 1b. Permission-dialog auto-approver.
# --------------------------------------------------------------------------
# seed-tcc.sh pre-grants what it CAN enumerate. Two classes it cannot:
#   * local network access, which is not a TCC service at all (no
#     kTCCServiceLocal* exists; grants live in the opaque nehelper store
#     /var/db/com.apple.networkextension.tracker-info), and
#   * any caller we did not think of — a microphone prompt was raised by a
#     helper whose display name was a bare string of digits, and a helper with
#     its own code identity inherits none of Unity.app's grants.
# Each miss costs 20+ minutes of a run polling in front of a dialog nobody can
# answer. So: a general approver that notices a permission dialog and presses
# the approving button, whatever asked.
#
# It drives through cua-driver's own stdio MCP (cua-driver-local mcp) — the component that
# ALREADY holds Accessibility and Screen Recording — so the approver process is
# an unprivileged HTTP client and adds no new granted identity to the image.
# Explicitly NOT AppleScript/System Events: that is what raised the
# two-minute Automation prompt pywinctl was removed for.
#
# It refuses to start outside a disposable Space VM (hw.model must be an Apple
# Virtual Machine). See the header of permission-approver.py.
#
# Started here rather than at build time, and KeepAlive, because it must be up
# for the life of the Space. It tolerates cua-driver not being bound yet: the
# first cycles just log a connect error and retry.
#
# The INTERPRETER matters. Discovery uses CGWindowListCopyWindowInfo rather than
# cua-driver's list_windows, because list_windows is deliberately layer-0 only
# and an alert panel need not be on layer 0 — so the driver can be blind to the
# very dialog we are here to press. CGWindowList has no layer filter and needs
# no permission for the metadata used (id, pid, owner, bounds). That call needs
# pyobjc, which /usr/bin/python3 does NOT have and the helper venv
# (install-agent-stack.sh) does. Fall back to the system python (and the layer-0 path) only if the venv
# is missing; the script says so loudly in its log when it does.
APPROVER="$HOME/.cua/permission-approver.py"
APPROVER_AGENT="$HOME/Library/LaunchAgents/com.trycua.permission-approver.plist"
APPROVER_PY="$HOME/.cua-server/venv/bin/python"
if [ ! -x "$APPROVER_PY" ] || ! "$APPROVER_PY" -c "import Quartz" 2>/dev/null; then
  APPROVER_PY=/usr/bin/python3
  echo "WARNING: helper venv python unusable; permission-approver falls back"
  echo "         to /usr/bin/python3 and layer-0-only window discovery"
fi
if [ -f "$APPROVER" ]; then
  cat > "$APPROVER_AGENT" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.trycua.permission-approver</string>
  <key>ProgramArguments</key>
  <array>
    <string>$APPROVER_PY</string><string>$APPROVER</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>$HOME/.cua/permission-approver.out.log</string>
  <key>StandardErrorPath</key><string>$HOME/.cua/permission-approver.err.log</string>
</dict>
</plist>
PLIST
  launchctl bootout "gui/$(id -u)/com.trycua.permission-approver" 2>/dev/null
  launchctl bootstrap "gui/$(id -u)" "$APPROVER_AGENT" 2>/dev/null
  echo "permission-approver armed (log: ~/.cua/permission-approver.log)"
else
  echo "WARNING: $APPROVER absent — permission dialogs will NOT be auto-approved"
fi

# --------------------------------------------------------------------------
# 2. No notification banners.
# --------------------------------------------------------------------------
# Notification Center banners are drawn over everything, steal nothing but the
# screen, and are impossible to pre-empt individually: the ones seen in a real
# run were "App Background Activity — 'bash' can run in the background"
# (persistent, whole run), "See what's new in macOS Tahoe", and "Quick Look
# Previewer Extension Added". Focus/Do-Not-Disturb assertions are stored in a
# format that changes between releases; unloading the agent that draws banners
# is version-independent and total. Nothing in a Space needs it.
if launchctl bootout "gui/$(id -u)/com.apple.notificationcenterui.agent" 2>/dev/null; then
  echo "notification center unloaded (no banners can be drawn)"
else
  echo "notification center already unloaded"
fi
/usr/bin/pkill -x NotificationCenter 2>/dev/null

# --------------------------------------------------------------------------
# 2b. No app resurrected by Resume.
# --------------------------------------------------------------------------
# A fresh clone came up with System Settings open on the General pane and
# nobody had touched it. This was assumed to be a side effect of the TCC
# prompts ("Open System Settings"); it is not. loginwindow relaunched it from
# the Resume list:
#   loginwindow: [com.apple.loginwindow.logging:TAL]
#     -[PersistentAppsSupport persistentAppPreLaunch] | --- Index:2,
#     bundleID:com.apple.systempreferences
# Whatever was open the last time the GOLDEN was shut down is baked into the
# image's per-host loginwindow preferences and reopens in every Space forever.
#
# Two parts, because by the time this script runs loginwindow has already done
# it: stop it happening again, and close what it just opened.
#
# The list is NOT in a ByHost com.apple.loginwindow plist — that file does not
# exist here, and `defaults -currentHost read com.apple.loginwindow` says the
# domain does not exist. loginwindow says where it really keeps it:
#   Container loaded from file:///Users/lume/Library/Group Containers/
#     group.com.apple.loginwindow.persistent-apps/persistantApps
# (Apple's spelling.) It rewrites that snapshot on a timer, not only at logout,
# so TALLogoutSavesState=false is what stops it coming back.
defaults write com.apple.loginwindow TALLogoutSavesState -bool false 2>/dev/null
defaults write -g NSQuitAlwaysKeepsWindows -bool false 2>/dev/null
PERSIST="$HOME/Library/Group Containers/group.com.apple.loginwindow.persistent-apps/persistantApps"
[ -e "$PERSIST" ] && { /bin/rm -rf "$PERSIST"; echo "cleared Resume app list"; }
/bin/rm -rf "$HOME/Library/Saved Application State"/* 2>/dev/null
# Blender and Unity Hub are on this list for a specific reason. Now that nothing
# launches them at boot (see section 6), Resume is the ONLY thing that can put
# them on a Space's desktop — and it does: every golden frozen while they were
# running has them baked into `persistantApps`, so a clone of such an image comes
# up with both windows open no matter what this script does or does not launch.
# Measured on a clone of cua-golden-md after the eager launches were removed:
# Blender pid 398 and Unity Hub pid 404, both parented to loginwindow, both up
# before this script ran, and the Hub without the `--disable-gpu` that section 6
# used to pass — the signature of a Resume launch rather than ours.
#
# Closing them here makes the clean-desktop guarantee a property of the running
# Space rather than of how the image happened to be frozen, which also means an
# older golden does not have to be rebuilt to benefit. It is safe at this point
# in the boot: nothing has asked for either app yet (the hot-load shim only
# launches Blender on a tools/call, which cannot have happened before the agent
# exists), and the teleport terminates the Hub itself before importing.
for app in "System Settings" "System Preferences" "Blender" "Unity Hub"; do
  /usr/bin/pkill -x "$app" 2>/dev/null && echo "closed $app (reopened by Resume)"
done

# --------------------------------------------------------------------------
# 3. Unity Hub onboarding markers.
# --------------------------------------------------------------------------
# The Hub rewrites every one of these on QUIT — during the golden build, and
# again when a teleport terminates and relaunches it — so writing them once is
# not enough. unity-hub-markers.sh puts them back, and a WatchPaths LaunchAgent
# re-runs it whenever the Hub touches its support directory.
HUB_SUPPORT="$HOME/Library/Application Support/UnityHub"
MARKER_AGENT="$HOME/Library/LaunchAgents/com.trycua.unity-hub-markers.plist"
# Written unconditionally, not `if [ ! -f ]`. The plist is part of the fix, so a
# golden that already carries an older one must be upgraded, not skipped.
if true; then
  mkdir -p "$(dirname "$MARKER_AGENT")" "$HUB_SUPPORT"
  cat > "$MARKER_AGENT" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.trycua.unity-hub-markers</string>
  <key>ProgramArguments</key>
  <array><string>/bin/bash</string><string>$HOME/.cua/unity-hub-markers.sh</string></array>
  <key>WatchPaths</key>
  <array><string>$HUB_SUPPORT</string></array>
  <!-- WatchPaths alone is not enough any more. It fires when the watched
       DIRECTORY changes, which covers the onboarding marker files written
       straight into it, but the Developer Data Framework banner suppression
       (see unity-hub-markers.sh) has to read an organization id out of
       logs/info-log.json and sentry/scope_v3.json — nested files the Hub
       appends to without touching the directory. So add a short interval as
       the backstop: the script is a no-op unless a value is actually wrong,
       so running it every 30s costs nothing and bounds how long the banner
       can be on screen after a teleport. -->
  <key>StartInterval</key><integer>30</integer>
  <key>StandardOutPath</key><string>$HOME/.cua/unity-hub-markers.log</string>
  <key>StandardErrorPath</key><string>$HOME/.cua/unity-hub-markers.log</string>
</dict>
</plist>
PLIST
fi
launchctl bootout "gui/$(id -u)/com.trycua.unity-hub-markers" 2>/dev/null
launchctl bootstrap "gui/$(id -u)" "$MARKER_AGENT" 2>/dev/null
"$HOME/.cua/unity-hub-markers.sh"
echo "Unity Hub onboarding markers written; watcher armed"

# --------------------------------------------------------------------------
# 3b. Unity Hub promo carousel.
# --------------------------------------------------------------------------
# The Hub's home screen cycles remote marketing banners — "Unity's legal terms
# are changing / Review terms", CoreCLR, Built-In Render Pipeline deprecation,
# "Enable Developer Data Framework" — which read as alerts on camera.
#
# There is no setting to turn it off. Every `hubDisable*` flag in the Hub's
# app.asar was enumerated (hubDisableSignInRequired, hubDisablePersonalLicense,
# hubDisableLearn, hubDisableSignin, hubDisableElevate, hubDisableCommunity,
# hubDisableVisualStudioDownload, hubDisableAutoUpdate, hubDisableWelcomeScreen,
# hubDisableAnalytics) and none of them covers banners; the carousel is not
# behind a feature flag either. The banner copy is not in the app at all — it is
# fetched at runtime by SanityBannersService.fetchBanners() from
#   https://fuvbjjlp.apicdn.sanity.io/v2022-03-07/data/query/hub-production
# (projectID `fuvbjjlp` from common-*.js, dataset `hub-production` from
# config-*.js). That host serves the Hub's CMS content and nothing else, and
# fetchBanners catches a failed fetch and returns [] — so a blocked host is a
# silent empty carousel, not an error state.
#
# Pre-seeding ~/Library/Application Support/UnityHub/dismissedBanners.json is
# the other supported route, but it needs each banner's id and goes stale the
# moment Unity publishes a new one.
HUB_CMS_HOST="fuvbjjlp.apicdn.sanity.io"
#
# NOTE the shape of the write. `... | sudo_run tee -a /etc/hosts` looks right
# and silently does nothing: sudo_run feeds the sudo password on stdin, so a
# pipe into it makes sudo eat the piped line as the password instead. Verified —
# it printed the success line while /etc/hosts was untouched. Let the elevated
# shell do the redirect.
if ! /usr/bin/grep -q "$HUB_CMS_HOST" /etc/hosts 2>/dev/null; then
  if sudo_run /bin/sh -c "printf '0.0.0.0 %s\n' '$HUB_CMS_HOST' >> /etc/hosts" \
     && /usr/bin/grep -q "$HUB_CMS_HOST" /etc/hosts; then
    echo "blocked Unity Hub banner CMS ($HUB_CMS_HOST)"
  else
    echo "WARNING: could not block $HUB_CMS_HOST in /etc/hosts"
  fi
else
  echo "Unity Hub banner CMS already blocked"
fi

# --------------------------------------------------------------------------
# 4. Unity Editor preferences that gate first-run windows.
# --------------------------------------------------------------------------
# MCP for Unity and FishNet both open an editor window on first load. Neither
# ships in the image — they are packages of whatever project the user opens —
# but EditorPrefs are per-USER, not per-project, so the image can pre-answer
# them for a project that does not exist yet. Both packages use the plain
# two-argument EditorPrefs overloads, so the keys are stored verbatim.
#
#   MCPForUnity.SetupCompleted / SetupDismissed
#       gate the two-step "MCP Setup" wizard (Next -> Configure Selected -> OK)
#       and, with it, the "Client Configuration — N configured, 0 failed" dialog
#       that only ever fires from inside that wizard
#       (Editor/Setup/SetupWindowService.cs:51-55).
#   MCPForUnity.AutoStartOnLoad = false
#       deliberately OFF. The agent talks to Unity through the official
#       `unity mcp` CLI (install-unity-cli.sh), so the third-party package must
#       not also stand up a server: two owners of one endpoint, and the
#       package's version is whatever the cloned project's manifest resolves
#       from `...unity-mcp.git#main` on the day. Suppressing its window is all
#       the image wants from it.
#   MCPForUnity.HttpServerLaunchConfirmed
#       suppresses the one-time "confirm launching an external process" toast
#       in case anything does reach that path
#       (Editor/Services/ServerManagementService.cs:299).
#   ShowedFishNetGettingStarted / ReminderEnabled
#       the FishNetGettingStartedEditor window and its periodic
#       "Have you considered leaving us a review?" nag.
for kv in \
  "MCPForUnity.SetupCompleted           TRUE" \
  "MCPForUnity.SetupDismissed           TRUE" \
  "MCPForUnity.AutoStartOnLoad          FALSE" \
  "MCPForUnity.HttpServerLaunchConfirmed TRUE" \
  "ShowedFishNetGettingStarted          TRUE" \
  "ReminderEnabled                      FALSE"; do
  set -- $kv
  defaults write com.unity3d.UnityEditor5.x "$1" -bool "$2" 2>/dev/null
done
defaults write com.unity3d.UnityEditor5.x MCPForUnity.HttpUrl -string "http://127.0.0.1:8080" 2>/dev/null
echo "wrote MCP-for-Unity + FishNet EditorPrefs"

# --------------------------------------------------------------------------
# 6. Applications are NOT launched here.
# --------------------------------------------------------------------------
# This section used to launch Unity Hub and Blender at every boot of every
# Space, unconditionally. That put Blender on screen for a user who never
# mentioned 3D, and it is the wrong shape: a Space should ADVERTISE what it can
# do and start an application only when an agent actually needs it.
#
# Blender: hot-loaded. ~/.cua/mcp-lazy-app.py fronts the blender MCP (see
# write-agent-mcp.sh). It serves initialize/tools/list with Blender closed, and
# on the first tools/call launches Blender and waits for 127.0.0.1:9876 to
# answer before forwarding. Readiness is the socket, not the process -- a
# Resume-launched Blender passes pgrep with the socket closed, which is the bug
# the old guard here had. The capability stays visible the whole time, because
# discovery lives in the get-skills MCP and the skills, not in a running app.
#
# Unity Hub: not launched here either, and it does NOT need to be. Measured
# constraint, so it is not re-derived: the teleport does the launching. The
# spacesd's teleport import terminates the Hub if it is up, imports accounts.db and the keychain
# items, and launches the Hub itself -- that launch is the signed-in one, and
# relaunching it yourself makes the
# dying instance flush its signed-out in-memory session over the fresh
# accounts.db. So a Hub started here is at best redundant work for the teleport
# to undo. Everything the Hub's first-ever run creates (app-support dirs,
# com.unity3d.* prefs) was created during the golden BUILD and ships in the
# image; nothing about it is per-boot. When an agent needs the Hub without a
# teleport, it launches it -- `launch_app` via cua-driver, or
# `open -a "Unity Hub" --args --disable-gpu` (the flag matters: the paravirtual
# Metal GPU process hangs the Hub otherwise). The unity skill says so.
#
# What DOES stay per-boot is the stale-lock cleanup: a Singleton left by the
# image's last boot makes the Hub refuse to launch, whoever launches it.
/bin/rm -f "$HUB_SUPPORT"/Singleton* 2>/dev/null
echo "cleared stale Unity Hub Singleton lock; no applications launched (apps are hot-loaded on demand)"

echo "=== $(date) prepare done ==="
