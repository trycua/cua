#!/bin/bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# seed-tcc.sh — pre-grant the TCC permissions the demo apps ask for, so no
# permission dialog can ever block (or blemish) an unattended agent run.
#
# TCC prompts are drawn by a hardened system process and deliberately IGNORE
# synthetic clicks, so an agent can never dismiss one — the grant must already
# be in the database before the app launches. Unity asks for Microphone at
# editor startup (FMOD enumerates input devices) and otherwise blocks forever
# on "Open Project: Initialize Asset Database".
#
# A row for a signed .app (client_type=0) is only honoured when its `csreq`
# matches the app's designated code requirement; a NULL csreq is ignored and the
# prompt still appears. Derive it per app rather than inserting NULL.
#
# Requires SIP disabled (true on the Lume golden).
#
#   seed-tcc.sh              seed every grant (build time)
#   seed-tcc.sh --refresh    only bump the timestamps on rows that already
#                            exist, then restart tccd (per boot)
#
# THE "BYPASS THE PRIVATE WINDOW PICKER" ALERT IS NOT IN TCC AT ALL.
#   "RCDP Host" is requesting to bypass the system private window picker and
#   directly access your screen and audio.
# kept appearing even though `kTCCServiceScreenCapture -> com.trycua.rcdphost`
# was present in the SYSTEM db with auth_value=2 and a valid csreq. That is
# correct and it is not the mechanism. The earlier conclusion — that this is
# ScreenCaptureKit's direct-capture alert with no separate service key, and that
# bumping `last_reminded` on the TCC row restarts its clock — is HALF right: the
# first half is true, the second half is wrong. The alert's schedule does not
# live in TCC.db. It lives in a per-user ReplayKit ledger:
#
#   ~/Library/Group Containers/group.com.apple.replayd/ScreenCaptureApprovals.plist
#
# keyed by bundle id, with (observed on a failing clone):
#   kScreenCaptureApprovalLastAlerted = <when the alert was last shown>
#   kScreenCapturePrivacyHintDate     = <when it will be shown NEXT>
#   kScreenCapturePrivacyHintPolicy   = 2592000   (30 days, in seconds)
#   kScreenCaptureAlertableUsageCount / kScreenCaptureApprovalLastUsed
#
# On a freshly cloned Space that file does not exist yet, so the FIRST capture
# has no future hint date, and the alert fires immediately — 83 s after boot in
# the observed run, and then sits there for the whole demo because a TCC/
# ReplayKit alert ignores synthetic clicks. No amount of TCC seeding prevents
# it; the ledger has to exist with a hint date in the future before the first
# capture. seed_screen_capture_approvals() below writes exactly that, at build
# time AND on every boot.
set -u

REFRESH=0
[ "${1:-}" = "--refresh" ] && REFRESH=1

DB="$HOME/Library/Application Support/com.apple.TCC/TCC.db"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# Two databases are involved:
#   * USER db (this $DB) — Microphone, Camera. Unity asks for Microphone at
#     editor startup (FMOD enumerates input devices) and otherwise blocks
#     forever on "Initialize Asset Database".
#   * SYSTEM db — Accessibility and Screen Recording, which are what let
#     cua-driver and cua-spacesd SEE and DRIVE the GUI. Without them an agent gets an
#     empty AX tree, check_permissions reports accessibility:false /
#     blocked_by_screen_recording, and macOS parks a universalAccessAuthWarn nag
#     on screen that nothing in a Space can dismiss.
SERVICES="kTCCServiceMicrophone kTCCServiceCamera"
SYSTEM_SERVICES="kTCCServiceAccessibility kTCCServiceScreenCapture"
SYSTEM_DB="/Library/Application Support/com.apple.TCC/TCC.db"
DAEMON_APPS="/Applications/CuaDriverLocal.app
/Applications/Cua Spacesd.app"

# sudo cannot prompt over SSH; feed the password in.
sudo_run() { printf '%s\n' "${CUA_SUDO_PW:-lume}" | sudo -S -p '' "$@"; }

# --- Automation / Apple Events (kTCCServiceAppleEvents) --------------------
#
# READ THIS FIRST: THESE ROWS ARE NOT WHAT KEEPS THE DIALOG OFF SCREEN.
# Everything below about the row shape is correct and was read off tccd's own
# rows rather than guessed — and it still does not work. Retested on a
# cold-booted fresh clone, with flags 0 and 1, auth_version 1 and 2, client
# csreq present and NULL, and client_type 1 and 0: tccd logs
#   Prompting for access to indirect object System Events by python3.13
# every single time. A path-keyed AppleEvents grant is not honoured on macOS
# 26.3; the supported route is a PPPC configuration profile, which needs MDM.
#
# What actually fixed it was removing the caller: the image no longer ships
# computer-server (whose pywinctl dependency was the only Apple Events caller);
# its helper venv carries only pyobjc's Quartz bindings.
#
# These rows are kept anyway, for one narrow reason that IS worth having: a
# prompt that is dismissed or times out leaves a sticky auth_value=0 denial at
# this primary key, which the image would then inherit and apply to anything
# attributed to the same client forever. Rewriting them to auth_value=2 on every
# boot clears that. Treat them as hygiene, not as the fix.
#
# Two "wants access to control System Events" dialogs blocked a whole demo run
# — one naming "python3.13" and one naming "sshd-keygen-wrapper" — each hanging
# its caller for ~2 minutes and then sitting on screen for the rest of the run.
# Like every TCC prompt they ignore synthetic clicks, so the grant has to exist
# before the call.
#
# WHERE THE APPLE EVENT CAME FROM. It was not hand-written AppleScript anywhere
# in our code, and it was not the cua-driver backend:
#   * computer_server (since removed from the image) enumerated windows through
#     Quartz (CGWindowListCopyWindowInfo in handlers/macos.py), with no Apple
#     Events;
#   * cua-driver lists apps through NSWorkspace and says so in a comment
#     ("stays entirely inside AppKit so listing ... never triggers the macOS
#     Automation permission for System Events"); its only osascript use is
#     Safari `do JavaScript`, which targets Safari, not System Events.
# The caller was a DEPENDENCY: computer_server's GenericWindowHandler was built
# on `pywinctl`, and pywinctl's macOS backend is AppleScript from top to bottom
# (33 `tell application "System Events"` blocks in _pywinctl_macos.py). Its MCP
# tool `computer_get_app_windows` took an app NAME, which routed to that
# generic handler, so it shelled out to osascript. Confirmed against the failing
# run: the agent called computer_get_app_windows at 04:53:26 and got nothing
# back until 04:55:29, the two minutes the prompt was up. Short of patching a
# pinned pip dependency there was no non-AppleEvents path for that tool, which
# is why computer-server was dropped rather than worked around. (Its pid-taking
# macOS handler was AX-only.)
# The second client is the SSH identity: tccd attributes an Apple Event to the
# RESPONSIBLE process, which for anything started from an SSH session is
# /usr/libexec/sshd-keygen-wrapper. The in-Space agent ran
#   osascript -e 'tell application "System Events" to set visible of process
#                 "Terminal" to false'
# and that is the name the second dialog carried.
#
# ROW SHAPE. AppleEvents is not a two-column service. The access table's primary
# key is (service, client, client_type, indirect_object_identifier), so a row is
# keyed by the requesting client AND the target, and the target needs both
#   indirect_object_identifier       = the target bundle id
#   indirect_object_identifier_type  = 0   (bundle id)
#   indirect_object_code_identity    = the target's csreq blob
# A plain (service, client) INSERT silently creates a useless row for the
# 'UNUSED' target. The shape here was read off the two DENIAL rows the failing
# run left behind rather than guessed, and matches them field for field except
# auth_value. The target csreq is generated, not copied: `csreq -r` on
#   identifier "com.apple.systemevents" and anchor apple
# reproduces the exact 52-byte blob tccd itself wrote.
#
# client_type=1 (absolute path), not 0 (bundle id) — these clients are plain
# executables.
#
# THE CLIENT csreq IS MANDATORY. A path-keyed row with csreq NULL is NOT
# honoured — tested on a live Space: rows present at auth_value=2 with NULL
# csreq, tccd restarted, and `osascript -e 'tell application "System Events"…'`
# over SSH put the "sshd-keygen-wrapper wants access to control System Events"
# dialog straight back on screen. The comment at the top of this file says a
# NULL csreq is ignored for signed .app bundles; it is ignored for path clients
# too. So derive it per binary:
#   codesign -d -r- <binary>   ->   designated => identifier "…" and anchor apple
# for Apple's signed sshd-keygen-wrapper, and for the ad-hoc-signed uv python
#   # designated => cdhash H"ab39…"
# — note the leading "# ", which marks an implicit requirement and which csreq
# will not compile, so it is stripped. Both compile to blobs identical byte for
# byte to what tccd itself wrote in the denial rows (verified).
#
# The cdhash goes stale when uv fetches a new patch release of python. That is
# survivable precisely because this runs on EVERY BOOT (--refresh), recomputing
# the requirement from whatever binary is on disk, and because every uv cpython
# on the box is granted rather than just the one the venv currently points at.
#
# INSERT OR REPLACE, not INSERT: a dismissed or timed-out prompt leaves a sticky
# auth_value=0 / auth_reason=9 denial at exactly this primary key, which is
# inherited by the image, and a plain INSERT would leave it in place.
AE_SERVICE="kTCCServiceAppleEvents"
AE_TARGETS="com.apple.systemevents"

# Resolve the executables that need the grant. The helper-venv interpreter
# is found through its venv symlink and realpath'd, because TCC keys on the real
# path; every other uv cpython 3.13 on the box is granted too so that rebuilding
# the venv onto a different patch release does not reintroduce the prompt.
apple_events_clients() {
  /usr/bin/python3 -c 'import os,sys;print(os.path.realpath(sys.argv[1]))' \
    "$HOME/.cua-server/venv/bin/python" 2>/dev/null
  ls -d "$HOME"/.local/share/uv/python/cpython-3.1*/bin/python3.1? 2>/dev/null
  # The system python an agent reaches for when it writes a helper script.
  echo /usr/bin/python3
  echo /usr/libexec/sshd-keygen-wrapper
}

# Compile a binary's designated requirement to a csreq blob, as hex.
# The `# ` prefix on an implicit (ad-hoc) requirement is stripped: csreq reads
# it as a comment, ends up with an empty file, and fails with
# "unexpected end of file".
client_csreq_hex() {
  codesign -d -r- "$1" 2>/dev/null \
    | sed -n 's/^# *designated => //p; s/^designated => //p' > "$TMP/cl.txt"
  [ -s "$TMP/cl.txt" ] || return 1
  /usr/bin/csreq -r "$TMP/cl.txt" -b "$TMP/cl.bin" 2>/dev/null || return 1
  xxd -p "$TMP/cl.bin" | tr -d '\n'
}

# --- Claude Code's own Microphone grant ------------------------------------
#
# THE "STRING OF DIGITS" MICROPHONE PROMPT IS CLAUDE CODE.
#   "2.1.269" would like to access the Microphone.
# The client is /Users/lume/.local/share/claude/versions/<version> — the real
# target of the ~/.local/bin/claude symlink. The binary is ad-hoc signed, so
# macOS has no bundle name to show and falls back to the last path component,
# which is the VERSION NUMBER. That is the whole reason the asking process
# looked anonymous: it was never a mystery helper, it was the agent itself.
#
# It fires shortly after the in-Space agent starts work, not at boot, and a run
# that hits it stalls in front of it: one Unity instance sat at 0.0% CPU with no
# window for 10m47s while this dialog was up.
#
# A pre-grant beats clicking a dialog, so grant it — but the path carries the
# VERSION, so a hardcoded row rots the moment the CLI updates, and the ad-hoc
# cdhash changes with it. Both problems are solved the same way the uv-python
# AppleEvents rows are: resolve the symlink and recompute the requirement from
# whatever binary is on disk, on EVERY BOOT. Every sibling version directory is
# granted too, so an update that has already landed is covered before the agent
# next runs.
#
# client_type=1 (absolute path) — this is a bare executable, not a bundle.
claude_microphone_clients() {
  # The symlink's current target first: that is what actually runs.
  /usr/bin/python3 -c 'import os,sys;print(os.path.realpath(sys.argv[1]))' \
    "$HOME/.local/bin/claude" 2>/dev/null
  # Every installed version, so an update already on disk is pre-granted.
  ls -d "$HOME"/.local/share/claude/versions/* 2>/dev/null
}

seed_claude_microphone() {
  local client chex n=0
  while IFS= read -r client; do
    [ -n "$client" ] && [ -f "$client" ] && [ -x "$client" ] || continue
    chex=$(client_csreq_hex "$client") || {
      echo "skip Microphone (no requirement): $client"; continue; }
    /usr/bin/sqlite3 "$DB" "INSERT OR REPLACE INTO access
      (service,client,client_type,auth_value,auth_reason,auth_version,csreq,flags,
       last_modified,last_reminded)
      VALUES ('kTCCServiceMicrophone','$client',1,2,2,1,X'$chex',0,
       strftime('%s','now'),strftime('%s','now'));" 2>/dev/null \
      && { echo "granted kTCCServiceMicrophone: $client"; n=$((n+1)); }
  done <<EOF
$(claude_microphone_clients | sort -u)
EOF
  [ "$n" = 0 ] && echo "note: no Claude Code binary found to grant Microphone to"
  return 0
}

seed_apple_events() {
  local target thex client chex n=0
  for target in $AE_TARGETS; do
    printf 'identifier "%s" and anchor apple\n' "$target" > "$TMP/ae.txt"
    /usr/bin/csreq -r "$TMP/ae.txt" -b "$TMP/ae.bin" 2>/dev/null || {
      echo "WARNING: csreq failed for $target"; continue; }
    thex=$(xxd -p "$TMP/ae.bin" | tr -d '\n')
    while IFS= read -r client; do
      [ -n "$client" ] && [ -e "$client" ] || continue
      chex=$(client_csreq_hex "$client") || {
        echo "skip AppleEvents (no requirement): $client"; continue; }
      /usr/bin/sqlite3 "$DB" "INSERT OR REPLACE INTO access
        (service,client,client_type,auth_value,auth_reason,auth_version,csreq,
         policy_id,indirect_object_identifier_type,indirect_object_identifier,
         indirect_object_code_identity,flags,last_modified,last_reminded)
        VALUES ('$AE_SERVICE','$client',1,2,2,1,X'$chex',
         NULL,0,'$target',X'$thex',1,
         strftime('%s','now'),strftime('%s','now'));" 2>/dev/null \
        && { echo "granted AppleEvents: $client -> $target"; n=$((n+1)); }
    done <<EOF
$(apple_events_clients | sort -u)
EOF
  done
  [ "$n" = 0 ] && echo "WARNING: no AppleEvents grants written"
  return 0
}

# PKILL DOES NOT RESTART tccd. Every earlier revision of this file ended with
#   pkill -f "TCC.framework/Support/tccd"    &&    echo "tccd restarted"
# and the message was a lie: the unprivileged form fails outright with
# "Operation not permitted", and even `sudo pkill` returns 0 while the process
# survives (tccd is signal-protected; SIP being off does not change that).
# Verified on a live Space — the user tccd kept the same pid across both, so its
# cached answers, including the sticky AppleEvents denial, were never dropped
# and no amount of correct database seeding took effect until the next boot.
#
# launchctl kickstart -k is what actually does it: same call, and the pid
# changed. Both domains, because Accessibility/ScreenCapture live in the system
# db served by the system tccd and the rest in the per-user one.
#
# BOTH DOMAINS, ON EVERY BOOT. An earlier revision restarted only the per-user
# tccd on the --refresh path, on the stated premise that "every row this function
# exists to publish lives in the per-user db served by the per-user tccd". That
# premise is wrong, and it is contradicted twelve lines above and at the top of
# this file: Accessibility and Screen Recording — the two grants cua-driver and
# cua-spacesd cannot see or drive the GUI without — live in the SYSTEM db, served by
# the SYSTEM tccd. Restarting only the user tccd leaves the one daemon whose
# cache actually gates the driver untouched.
#
# That omission is what made roughly half of all clones dead on arrival. The
# system tccd answers a client's first Accessibility/ScreenCapture query and
# caches the answer; the refresh below rewrites the system db with sqlite3
# BEHIND that daemon's back, so a cached denial is never dislodged. cua-driver's
# permission gate then polls a (correctly fresh, out-of-process) probe once a
# second for its ten-minute deadline, is told "denied" every time by the stale
# system tccd, gives up, and latches PERMISSION_GATE_PENDING for the life of the
# process — every tool call after that returns
#   permissions_pending ... error_code='75'
# while `sqlite3 "$SYSTEM_DB"` shows both rows present at auth_value=2. That is
# exactly the state a failing clone was found in, and it is also why every
# recovery attempted on it failed: restarting the DRIVER cannot help when the
# stale cache is in the system tccd, `killall tccd` is a no-op (tccd is
# signal-protected, see above), and re-running prepare-space restarted only the
# user tccd again.
#
# The old comment's caution was about restarting a system daemon early in the
# login window. That cost is real but small and one-off; a permanently unusable
# Space is neither. `pkill -x UserNotificationCenter` stays build-time-only —
# that one is about clearing a prompt already drawn, which a fresh clone has not
# got.
restart_tcc_clients() {
  local before after sysbefore sysafter
  before=$(/usr/bin/pgrep -f 'TCC.framework/Support/tccd$' | head -1)
  launchctl kickstart -k "gui/$(id -u)/com.apple.tccd" >/dev/null 2>&1
  sleep 2
  after=$(/usr/bin/pgrep -f 'TCC.framework/Support/tccd$' | head -1)
  if [ -n "$after" ] && [ "$before" != "$after" ]; then
    echo "user tccd restarted (pid $before -> $after)"
  else
    echo "WARNING: user tccd was NOT restarted (pid $after); grants take effect next boot"
  fi
  # The system tccd runs as root and is the one that serves kTCCServiceAccessibility
  # and kTCCServiceScreenCapture. Identify it by its euid, not by the executable
  # path — both daemons run the same binary.
  #
  # THE LABEL IS com.apple.tccd.system, NOT com.apple.tccd. This is the whole
  # bug. An earlier revision ran `launchctl kickstart -k system/com.apple.tccd`
  # at build time and, because the output was discarded, nobody saw it answer
  #   Could not find service "com.apple.tccd" in domain for system
  # every single time. There is no such service in the system domain: `launchctl
  # print system | grep tccd` shows one entry, com.apple.tccd.system. So the
  # system tccd has never once been restarted by this script — not at build
  # time, not at boot — and the seeded Accessibility and Screen Recording grants
  # only ever took effect when it happened to be restarted by something else.
  # Verified on a live failing clone: pid 153 was still the original system tccd
  # three minutes into the boot; kickstarting the correct label moved it to 1714,
  # and the driver went from "❌ not granted" to "✅ granted" on the next restart.
  # NO `$` ANCHOR HERE. The per-user tccd is invoked as a bare path, so the
  # pattern above anchors on it correctly — but the system one is invoked as
  #   /System/Library/PrivateFrameworks/TCC.framework/Support/tccd system
  # with a literal `system` argument, so the anchored pattern never matches it
  # and the check reported "system tccd was NOT restarted (pid )" on a boot where
  # it demonstrably had been (pid moved, and the driver saw its grants).
  sysbefore=$(/usr/bin/pgrep -u 0 -f 'TCC.framework/Support/tccd' | head -1)
  sudo_run launchctl kickstart -k system/com.apple.tccd.system >/dev/null 2>&1
  sleep 2
  sysafter=$(/usr/bin/pgrep -u 0 -f 'TCC.framework/Support/tccd' | head -1)
  if [ -n "$sysafter" ] && [ "$sysbefore" != "$sysafter" ]; then
    echo "system tccd restarted (pid $sysbefore -> $sysafter)"
  else
    echo "WARNING: system tccd was NOT restarted (pid $sysafter); Accessibility and" \
         "Screen Recording may still answer from a stale cache"
  fi
  if [ "$REFRESH" = 0 ]; then
    # Build time only. A prompt already drawn keeps its own modal session and
    # serialises every later Apple Event behind it.
    sudo_run /usr/bin/pkill -x UserNotificationCenter 2>/dev/null || true
  fi
  # Anything that asked while a denial stood cached the refusal in-process.
  { launchctl kickstart -k "gui/$(id -u)/com.trycua.spacesd" >/dev/null 2>&1 \
    || launchctl kickstart -k "gui/$(id -u)/com.trycua.guestd" >/dev/null 2>&1 \
    || launchctl kickstart -k "gui/$(id -u)/com.trycua.env_driver" >/dev/null 2>&1; } \
    && echo "restarted cua-spacesd"  # com.trycua.guestd, com.trycua.env_driver: older goldens
}

# --- ReplayKit screen-capture approval ledger -------------------------------
# See the long note at the top. This is what actually suppresses
#   "<app> is requesting to bypass the system private window picker".
# Every capturing bundle id gets a hint date far in the future and a policy
# interval long enough that macOS never rearms it within the life of an image.
SCAP_PLIST="$HOME/Library/Group Containers/group.com.apple.replayd/ScreenCaptureApprovals.plist"
SCAP_BIDS="com.trycua.driver.local
com.trycua.cua-env-driver
com.apple.screensharing.agent"
# 100 years, in seconds — larger than any Space's lifetime.
SCAP_POLICY=3153600000

# The file is rewritten whole rather than patched key by key: `plutil -replace`
# takes a dot-separated KEYPATH and every key here is a bundle id, so a keypath
# would be parsed as nesting. Writing XML and converting is unambiguous.
# --- stale SSH Accessibility denial ----------------------------------------
# The "…control this Mac" panel seen in the last demo run was NOT caused by a
# missing grant for any demo binary. It was caused by a debugging one-liner run
# by hand over SSH:
#   sudo launchctl asuser 501 /usr/bin/python3 /tmp/axman.py
# (axman.py sets AXManualAccessibility on Unity Hub's pid). tccd attributes an
# AX call to the RESPONSIBLE process, which for anything started from an SSH
# session is /usr/libexec/sshd-keygen-wrapper — so the panel named the SSH
# identity, nobody could click it, and tccd wrote
#   kTCCServiceAccessibility | /usr/libexec/sshd-keygen-wrapper | auth_value=0
# Nothing the image ships does this, so there is no grant to add. What is worth
# doing is removing the sticky DENY, because it is inherited by the image and
# would silently refuse any future AX work attributed to an SSH session.
# Deleted, not flipped to 2: granting Accessibility to the SSH identity would
# hand full synthetic-input rights to anything anyone ever runs over SSH.
clear_ssh_accessibility_deny() {
  local n
  n=$(sudo_run /usr/bin/sqlite3 "$SYSTEM_DB" \
    "delete from access where service='kTCCServiceAccessibility'
       and client like '%sshd-keygen-wrapper%' and auth_value=0;
     select changes();" 2>/dev/null)
  [ "${n:-0}" != "0" ] && echo "removed stale SSH Accessibility denial (${n} row)"
  return 0
}

seed_screen_capture_approvals() {
  mkdir -p "$(dirname "$SCAP_PLIST")" 2>/dev/null || return 1
  now=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  future=$(date -u -v+100y +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || echo "2125-01-01T00:00:00Z")
  {
    echo '<?xml version="1.0" encoding="UTF-8"?>'
    echo '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">'
    echo '<plist version="1.0"><dict>'
    while IFS= read -r BID; do
      [ -n "$BID" ] || continue
      printf '<key>%s</key><dict>' "$BID"
      printf '<key>kScreenCaptureApprovalLastAlerted</key><date>%s</date>' "$now"
      printf '<key>kScreenCaptureApprovalLastUsed</key><date>%s</date>' "$now"
      printf '<key>kScreenCapturePrivacyHintDate</key><date>%s</date>' "$future"
      printf '<key>kScreenCapturePrivacyHintPolicy</key><integer>%s</integer>' "$SCAP_POLICY"
      printf '<key>kScreenCaptureAlertableUsageCount</key><integer>0</integer>'
      printf '</dict>\n'
    done <<EOF
$SCAP_BIDS
EOF
    echo '</dict></plist>'
  } > "$SCAP_PLIST.tmp"
  if /usr/bin/plutil -convert binary1 "$SCAP_PLIST.tmp" 2>/dev/null; then
    mv -f "$SCAP_PLIST.tmp" "$SCAP_PLIST"
    chmod 600 "$SCAP_PLIST"
    echo "screen-capture approvals seeded (next hint $future): $(printf '%s' "$SCAP_BIDS" | tr '\n' ' ')"
  else
    rm -f "$SCAP_PLIST.tmp"
    echo "WARNING: could not write $SCAP_PLIST"
  fi
}

APPS="/Applications/Unity/Hub/Editor/6000.3.9f1/Unity.app
/Applications/Unity Hub.app
/Applications/Blender.app"

if [ "$REFRESH" = 1 ]; then
  # Per boot: keep the existing rows (csreq and all) and only restart the
  # re-ask clock, then make tccd reload. Rewriting the rows here would need
  # codesign/csreq work on every boot for no gain.
  /usr/bin/sqlite3 "$DB" "UPDATE access SET last_modified=strftime('%s','now'),
    last_reminded=strftime('%s','now') WHERE auth_value=2;" 2>/dev/null \
    && echo "refreshed user TCC timestamps"
  sudo_run /usr/bin/sqlite3 "$SYSTEM_DB" "UPDATE access SET
    last_modified=strftime('%s','now'), last_reminded=strftime('%s','now')
    WHERE auth_value=2;" 2>/dev/null \
    && echo "refreshed system TCC timestamps"
  seed_screen_capture_approvals
  clear_ssh_accessibility_deny
  # AppleEvents rows are rewritten in full on every boot, not just timestamped.
  # They are cheap (no codesign work — the target csreq is generated from a
  # bundle id) and this is the only thing that clears a sticky auth_value=0 that
  # a previous Space's dismissed prompt baked into the image.
  seed_apple_events
  # Recomputed every boot on purpose: the client path carries Claude Code's
  # version and the ad-hoc cdhash moves with it, so a row written once rots.
  seed_claude_microphone
  sudo_run pkill -f universalAccessAuthWarn 2>/dev/null || true
  restart_tcc_clients
  exit 0
fi

seed_screen_capture_approvals
clear_ssh_accessibility_deny
seed_apple_events
seed_claude_microphone

while IFS= read -r APP; do
  [ -d "$APP" ] || { echo "skip (absent): $APP"; continue; }
  BID=$(/usr/bin/plutil -extract CFBundleIdentifier raw "$APP/Contents/Info.plist" 2>/dev/null)
  [ -n "$BID" ] || { echo "skip (no bundle id): $APP"; continue; }

  # Strip the `# ` an AD-HOC signature prefixes its designated requirement with
  # (`# designated => cdhash H"..."`). Without it an ad-hoc-signed bundle — what
  # _install-local-rust.sh produces unless a stable signing identity exists —
  # was reported "skip (unsigned)" and silently got NO grant at all.
  REQ=$(codesign -d -r- "$APP" 2>/dev/null | sed -n 's/^# *designated => //p; s/^designated => //p')
  [ -n "$REQ" ] || { echo "skip (unsigned): $APP"; continue; }
  # csreq -r takes a FILE containing the requirement, not an inline string.
  printf '%s\n' "$REQ" > "$TMP/req.txt"
  /usr/bin/csreq -r "$TMP/req.txt" -b "$TMP/req.bin" 2>/dev/null || {
    echo "skip (csreq failed): $APP"; continue; }
  HEX=$(xxd -p "$TMP/req.bin" | tr -d '\n')

  for SVC in $SERVICES; do
    /usr/bin/sqlite3 "$DB" "INSERT OR REPLACE INTO access
      (service,client,client_type,auth_value,auth_reason,auth_version,csreq,flags,last_modified,last_reminded)
      VALUES ('$SVC','$BID',0,2,2,1,X'$HEX',0,strftime('%s','now'),strftime('%s','now'));" \
      && echo "granted $SVC -> $BID"
  done
done <<EOF
$APPS
EOF

# --- system-level grants for the driver daemons ----------------------------
while IFS= read -r APP; do
  [ -d "$APP" ] || { echo "skip (absent): $APP"; continue; }
  BID=$(/usr/bin/plutil -extract CFBundleIdentifier raw "$APP/Contents/Info.plist" 2>/dev/null)
  [ -n "$BID" ] || { echo "skip (no bundle id): $APP"; continue; }
  # Strip the `# ` an AD-HOC signature prefixes its designated requirement with
  # (`# designated => cdhash H"..."`). Without it an ad-hoc-signed bundle — what
  # _install-local-rust.sh produces unless a stable signing identity exists —
  # was reported "skip (unsigned)" and silently got NO grant at all.
  REQ=$(codesign -d -r- "$APP" 2>/dev/null | sed -n 's/^# *designated => //p; s/^designated => //p')
  [ -n "$REQ" ] || { echo "skip (unsigned): $APP"; continue; }
  printf '%s\n' "$REQ" > "$TMP/sreq.txt"
  /usr/bin/csreq -r "$TMP/sreq.txt" -b "$TMP/sreq.bin" 2>/dev/null || {
    echo "skip (csreq failed): $APP"; continue; }
  SHEX=$(xxd -p "$TMP/sreq.bin" | tr -d '\n')
  for SVC in $SYSTEM_SERVICES; do
    # INSERT OR REPLACE also flips an existing *denial* (auth_value 0) to
    # allowed — a denied Accessibility row is exactly what a dismissed prompt
    # leaves behind, and it is sticky.
    sudo_run /usr/bin/sqlite3 "$SYSTEM_DB" "INSERT OR REPLACE INTO access
      (service,client,client_type,auth_value,auth_reason,auth_version,csreq,flags,last_modified,last_reminded)
      VALUES ('$SVC','$BID',0,2,2,1,X'$SHEX',0,strftime('%s','now'),strftime('%s','now'));" \
      && echo "granted $SVC -> $BID (system)"
  done
done <<EOF
$DAEMON_APPS
EOF

# A prompt already on screen keeps its stale answer; clear it.
sudo_run pkill -f universalAccessAuthWarn 2>/dev/null || true

echo "--- seeded rows ---"
/usr/bin/sqlite3 "$DB" \
  "select service,client,auth_value,length(csreq) from access where csreq is not null and (client like '%unity%' or client like '%blender%');"
sudo_run /usr/bin/sqlite3 "$SYSTEM_DB" \
  "select service,client,auth_value,length(csreq) from access where client like '%trycua%';" 2>/dev/null
/usr/bin/sqlite3 "$DB" \
  "select service,client,client_type,auth_value,indirect_object_identifier,
          length(indirect_object_code_identity)
     from access where service='$AE_SERVICE';"

# tccd caches the database; restart it so the new grants take effect.
restart_tcc_clients

# --- prove the grants actually landed --------------------------------------
#
# Every write above is best-effort: a failed sqlite3 prints to stderr and the
# loop moves on. Until this check existed the script's exit status was just
# restart_tcc_clients', so a build on a SIP-ENABLED guest — where EVERY write
# fails with "authorization denied" — still reported success and shipped an
# image with no grants at all. Nothing noticed until an agent got an empty AX
# tree mid-run. Verify by reading the databases back.
if [ "$REFRESH" -eq 0 ]; then
  MISSING=""
  for BID in com.trycua.driver.local com.trycua.cua-env-driver; do
    for SVC in $SYSTEM_SERVICES; do
      n=$(sudo_run /usr/bin/sqlite3 "$SYSTEM_DB" \
        "select count(*) from access where service='$SVC' and client='$BID'
           and auth_value=2 and csreq is not null;" 2>/dev/null | tr -d '[:space:]')
      [ "${n:-0}" = "1" ] || MISSING="$MISSING $SVC->$BID"
    done
  done
  n=$(/usr/bin/sqlite3 "$DB" \
    "select count(*) from access where service='kTCCServiceMicrophone'
       and auth_value=2 and csreq is not null;" 2>/dev/null | tr -d '[:space:]')
  [ "${n:-0}" -ge 1 ] 2>/dev/null || MISSING="$MISSING kTCCServiceMicrophone(user db)"

  if [ -n "$MISSING" ]; then
    echo "" >&2
    echo "FAILED: these TCC grants are not in the database after seeding:$MISSING" >&2
    if [ "$(csrutil status 2>/dev/null)" != "System Integrity Protection status: disabled." ]; then
      echo "" >&2
      echo "  SIP is not disabled in this guest. TCC.db is SIP-protected, so no" >&2
      echo "  grant can be written. Turn SIP off in the guest first." >&2
    fi
    exit 1
  fi
  echo "verified: all system + user TCC grants present"
fi
