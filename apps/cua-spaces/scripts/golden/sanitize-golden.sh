#!/bin/bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# sanitize-golden.sh — strip every trace of the building machine's IDENTITY and
# WORK from the image, so the golden is publishable and each user's Space gets
# its identity solely from teleport.
#
# Run this as the LAST step of the golden build, before the final stop/freeze.
# It is idempotent and safe to re-run.
#
# The golden must ship apps + configuration + skills. It must NOT ship:
#   - credentials of any kind (Unity account, UVCS, Claude, browser profiles)
#   - a Unity *license/entitlement*, which is an activation bound to one account
#   - a checked-out project (biome-tiles is cloned live from Unity Version
#     Control through the teleported Hub login — that is the demo's whole point)
set -u

say() { printf '\033[1;36m==> %s\033[0m\n' "$*"; }
gone() { [ -e "$1" ] && { rm -rf "$1"; echo "  removed $1"; } || echo "  absent  $1"; }
sudo_run() { printf '%s\n' "${CUA_SUDO_PW:-lume}" | sudo -S -p '' "$@"; }

say "Unity: license/entitlement (an activation tied to the builder's account)"
gone "$HOME/Library/Unity/licenses"
gone "$HOME/Library/Application Support/Unity/Unity_lic.ulf"
gone "/Library/Application Support/Unity/Unity_lic.ulf"

say "Unity Hub: signed-in account"
for f in accounts.db accounts.db-shm accounts.db-wal; do
  gone "$HOME/Library/Application Support/UnityHub/$f"
done
# Keep the firstTime*/hide* onboarding markers: they suppress the "Get set up"
# wizard and carry no identity.

say "Unity Version Control (Plastic) client credentials"
gone "$HOME/.plastic4"

say "Credential-baking machinery (must not ship: it reinstalls a real token on every boot)"
gone "$HOME/.cua/unity-token.txt"
gone "$HOME/.cua/install-unity-token.sh"
# Replace the token-installing boot agent with the generic prepare-space one,
# which sets up the login keychain and launches the Hub but installs no secrets.
OLD_AGENT="$HOME/Library/LaunchAgents/com.trycua.prepare-demo.plist"
NEW_AGENT="$HOME/Library/LaunchAgents/com.trycua.prepare-space.plist"
if [ -f "$OLD_AGENT" ]; then
  launchctl bootout "gui/$(id -u)/com.trycua.prepare-demo" 2>/dev/null
  rm -f "$OLD_AGENT"; echo "  removed boot agent com.trycua.prepare-demo"
fi
if [ -x "$HOME/.cua/prepare-space.sh" ] && [ ! -f "$NEW_AGENT" ]; then
  cat > "$NEW_AGENT" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.trycua.prepare-space</string>
  <key>ProgramArguments</key>
  <array><string>/bin/bash</string><string>$HOME/.cua/prepare-space.sh</string></array>
  <key>RunAtLoad</key><true/>
</dict>
</plist>
PLIST
  echo "  installed boot agent com.trycua.prepare-space"
fi

say "Coding-agent credentials"
gone "$HOME/.claude.json"
# NOT `gone "$HOME/.claude"`. The whole directory used to be deleted, which also
# deleted ~/.claude/skills — the on-disk copy of the unity/blender skills that
# install-skills.sh puts there so an agent can read them without being launched
# with --mcp-config. The image is supposed to ship skills (see the header); it
# is credentials, history and per-project state that must not ship. So remove
# everything under ~/.claude EXCEPT skills/, and make sure the skills survived.
if [ -d "$HOME/.claude" ]; then
  find "$HOME/.claude" -mindepth 1 -maxdepth 1 ! -name skills -exec rm -rf {} + \
    && echo "  emptied $HOME/.claude (kept skills/)"
else
  echo "  absent  $HOME/.claude"
fi
gone "$HOME/.codex"

say "Browser profiles"
gone "$HOME/Library/Application Support/Google/Chrome"
gone "$HOME/Library/Application Support/Firefox"

say "Checked-out projects (cloned live from UVCS at demo time)"
gone "$HOME/biome-tiles"
# Any stray editor state that would point at a project that no longer exists.
gone "$HOME/Library/Application Support/UnityHub/projects-v1.json"
gone "$HOME/.unity-mcp"

say "Demo scaffolding that would let an agent shortcut the work"
# A pre-written hoverboard generator, probe scripts and stale outputs are not
# part of the image's contract. Leaving them invites an agent to run a canned
# script instead of modelling in Blender — the same way a pre-baked .fbx once
# made runs look clean while Blender was never used at all.
for f in make_hoverboard.py mcp_edit.py mcp_probe.py mcp_probe2.py demo-out.txt \
         add-demo-mcps.sh blender-install-fixed.sh install-unity-token.sh; do
  gone "$HOME/.cua/$f"
done
gone "$HOME/.cua/blendwork"

say "Keychain items holding teleported secrets"
# Every keychain on the search list, not just whichever one an unqualified
# lookup resolves first. A Space runs with two user keychains (`cua`, the
# default, and macOS's own `login`), and apps have minted items into both:
# Unity Hub wrote its "UnityHub Safe Storage" key into the login keychain on
# every boot of an image built before `cua` became the default, and an
# unqualified delete stops at the first match, leaving the other copy in the
# published image.
KEYCHAINS=$(security list-keychains -d user | tr -d '" ' )
for svc in unity "UnityHub Safe Storage" "Chrome Safe Storage" "Claude Code-credentials"; do
  cleared=0
  for kc in $KEYCHAINS; do
    while security delete-generic-password -s "$svc" "$kc" >/dev/null 2>&1; do cleared=1; done
  done
  # Unqualified too, to catch anything off the search list entirely.
  while security delete-generic-password -s "$svc" >/dev/null 2>&1; do cleared=1; done
  if [ "$cleared" = 1 ]; then
    echo "  cleared keychain service: $svc"
  else
    echo "  absent  keychain service: $svc"
  fi
done

say "Repairing /etc/kcpassword and the login keychain (THE fix for the unanswerable keychain dialog)"
# THE root cause of every "<App> wants to access key '<x>' in your keychain" and
# "<daemon> wants to use the '<kc>' keychain" panel a Space has ever shown, and
# it is one padding bug in the autologin password file.
#
# The symptom, measured in a booted Space in the Aqua session (`launchctl asuser
# 501`, because ssh lands in launchd's Background session where keychain reads
# fail as "not signed in"):
#
#   security add-generic-password -U -s probe -a p -w x -A login.keychain-db
#     -> ok
#   security set-generic-password-partition-list -S ... -k lume ... login.keychain-db
#     -> SecKeychainItemSetAccessWithPassword: The user name or passphrase you
#        entered is not correct.
#
# So the login keychain did not take the account password, even though `sshpass
# -p lume ssh` did. (`security unlock-keychain -p lume` succeeding proves
# NOTHING and is what misled two earlier investigations: unlocking an
# ALREADY-UNLOCKED keychain never looks at the password. Only a subcommand
# taking `-k` checks it.)
#
# The cause is /etc/kcpassword. It is the account password XOR a fixed 11-byte
# key, and the plaintext must be NUL-padded to a multiple of 12 BEFORE the XOR
# so the padding lands as key bytes and the decoder terminates on it. lume's
# offline unattended patcher XOR'd first and appended raw zeroes after, so for
# the password "lume" the file held
#   11 fc 3f 46 00 00 00 00 00 00 00 00      (wrong)
# instead of
#   11 fc 3f 46 d2 bc dd ea a3 b9 1f 7d      (right)
# and loginwindow decoded the autologin password as
#   "lume\xd2\xbc\xdd\xea\xa3\xb9\x1f\x7d".
#
# Autologin still SUCCEEDS with that value, so nothing looks broken — but
# loginwindow cannot unlock the login keychain with it. Per-boot, from the
# unified log:
#
#   SecKeychainLogin failed: -2147413984, password was supplied
#   loginResetLoginKeychainIfPossible | ERROR: Unable to get ALPW
#   Keychain could not be unlocked, local account, moving login keychain to
#     the side and creating a replacement
#   SecKeychainResetLogin: reset AKS passphrase
#
# EVERY BOOT. The replacement is keyed to an AKS passphrase, so its password is
# known to nobody, and any teleported secret landing in it makes the teleport
# importer's partition-list write fail and the destination app raise a password box that NO
# ONE ON EARTH can answer. It is also where the pile of login_renamed_N files
# comes from — this image had accumulated 22.
#
# The permanent fix is in lume (MacOSOfflineSetupPatcher.kcpasswordData), so new
# base VMs are correct. This repairs an image built before that fix, and is
# cheap and idempotent enough to keep as a guard afterwards.
#
# The previous "fix" routed around all of this instead: a second keychain
# (`cua`) with a known password, made the default, plus an unlock watchdog
# LaunchAgent, a search-list ordering dance and an ACL probe. ~200 lines that
# only worked on one code path in one order and left a plain provision broken.
# Deleted.
say "  /etc/kcpassword"
/usr/bin/python3 - <<'KCP' > /tmp/kcpassword.new
import sys
key = bytes.fromhex("7d895223d2bcddeaa3b91f")
pw = b"lume"
padded = pw + b"\x00" * (12 - len(pw) % 12)
sys.stdout.buffer.write(bytes(b ^ key[i % len(key)] for i, b in enumerate(padded)))
KCP
sudo_run /bin/cp /tmp/kcpassword.new /etc/kcpassword
sudo_run /usr/sbin/chown root:wheel /etc/kcpassword
sudo_run /bin/chmod 600 /etc/kcpassword
rm -f /tmp/kcpassword.new
echo "  /etc/kcpassword rewritten so loginwindow decodes the account password"

say "  login keychain"
# With kcpassword correct, loginwindow unlocks this keychain at autologin with
# the account password and ADOPTS it, instead of moving it aside. Creating it
# here rather than just deleting it means the password is known and verifiable
# at build time (see the check at the end of this script) rather than on trust.
#
# This is safe at BUILD time and would not be per boot: the image shuts down
# immediately after, so no session outlives the swap. A keychain swapped in
# under a session that has already opened the old one by identity reads as a
# different, locked keychain, and the first app to touch it raises "wants to use
# the 'login' keychain" — which is why prepare-space.sh does none of this.
KC="$HOME/Library/Keychains"
rm -f "$KC/login.keychain-db"* "$KC/login_renamed_"*.keychain-db
security create-keychain -p lume "$KC/login.keychain-db"
security default-keychain -d user -s "$KC/login.keychain-db"
security list-keychains -d user -s "$KC/login.keychain-db"
security unlock-keychain -p lume "$KC/login.keychain-db"
security set-keychain-settings "$KC/login.keychain-db"   # no auto-lock timeout
echo "  login.keychain-db re-created with the account password, default, unlocked"

# Everything downstream follows with no special ordering:
#   * login is the default keychain on any Mac, so Unity Hub's
#     @unity/hub-keyring — which resolves the DEFAULT keychain only and never
#     walks the search list — finds a teleported secret.
#   * login is first on the search list, so unqualified reads find it.
#   * cua-spacesd's teleport import needs no CUA_ENV_KEYCHAIN override: its
#     fallback is the default keychain, which is this one.
#   * no non-default keychain exists for a system daemon to mint a stale-ACL
#     item into, so the "<daemon> wants to use the 'cua' keychain" class is gone
#     at the source.

say "  removing the old cua.keychain workaround, if this image carries it"
rm -f "$KC/cua.keychain-db"* 2>/dev/null \
  && echo "  removed a legacy cua.keychain-db"
rm -f "$HOME/Library/LaunchAgents/com.trycua.keychain-unlock.plist" 2>/dev/null \
  && echo "  removed the legacy keychain-unlock watchdog LaunchAgent"
launchctl bootout "gui/$(id -u)/com.trycua.keychain-unlock" 2>/dev/null

say "Stale login_renamed_N keychains"
# loginwindow renames login.keychain-db aside whenever it cannot unlock it and
# creates a replacement. Every golden build boot that hit that path left another
# file behind and nothing ever collected them: the image had accumulated 22
# (login_renamed_1 .. login_renamed_22). They are dead weight in every clone and
# noise in exactly the directory we now need to be able to read at a glance.
renamed=$(ls -1 "$HOME/Library/Keychains/"login_renamed_*.keychain-db 2>/dev/null | wc -l | tr -d ' ')
if [ "${renamed:-0}" -gt 0 ]; then
  rm -f "$HOME/Library/Keychains/"login_renamed_*.keychain-db*
  echo "  removed $renamed stale login_renamed_N keychain(s)"
else
  echo "  none present"
fi
# Also drop the atomic-write scratch files `security` leaves behind on a crash.
rm -f "$HOME/Library/Keychains/"*.keychain-db.sb-* 2>/dev/null

say "Disabling Siri (a headless workspace has no use for it)"
# NOTE: this is not a keychain fix and must not be relied on as one. It was
# added when assistantd was believed to be the cause of the SecurityAgent
# panels; it is not. The keychain fix is the login-keychain re-key above.
# Keeping the Siri disable anyway: a
# Space runs Unity, Blender and a coding agent, so Siri is pure overhead, and
# fewer daemons touching the keychain is fewer chances to mint junk into it.
# Do NOT extend this list to Messages/iCloud/Spotlight hoping to chase the
# panel — ~25 daemons call SecItem on every login and most are load bearing.
#
# Historical rationale, retained because the observation is still accurate:
# assistantd raised the ~23-minute panel, and it reached the keychain for a reason
# that has nothing to do with this workspace: at 11:40:54.608 it logged
#   [com.apple.IDS:Registration] enabledAccountsForService
#     com.apple.private.alloy.siri.icloud
# and called SecItemCopyMatching one millisecond later — a Siri/iCloud account
# lookup, in a headless throwaway VM that has no iCloud account and no Siri.
#
# This MUST be baked into the image, not done by prepare-space.sh: assistantd
# asked at 11:40:54 and prepare-space.sh's first keychain call was 11:40:57.
# `launchctl disable` writes the override into
# /var/db/com.apple.xpc.launchd/disabled.501.plist, which is part of the image,
# so it takes effect from the very first login of a clone.
#
# Deliberately NOT disabling Spotlight (corespotlightd/mdworker): the agent
# stack and Unity both rely on Spotlight-backed lookups, and the search-list
# reset already removes its path to `cua`. Siri has no such claim.
for svc in com.apple.assistantd com.apple.assistant_service com.apple.siriknowledged \
           com.apple.sirittsd com.apple.siriactionsd com.apple.siriinferenced \
           com.apple.SiriTTSTrainingAgent com.apple.Siri.agent; do
  if launchctl disable "gui/$(id -u)/$svc" 2>/dev/null; then
    echo "  disabled: $svc"
  else
    echo "  (not present / not disableable): $svc"
  fi
done
# Belt and braces: turn Siri off in preferences too, so nothing re-enables the
# agents on a whim.
defaults write com.apple.assistant.support "Assistant Enabled" -bool false 2>/dev/null
defaults write com.apple.Siri StatusMenuVisible -bool false 2>/dev/null
defaults write com.apple.Siri VoiceTriggerUserEnabled -bool false 2>/dev/null

say "Resume state (whatever was open when the golden was shut down)"
# loginwindow reopens the golden's last-open apps in every Space cloned from it
# — a fresh clone came up with System Settings on screen for exactly this
# reason. prepare-space.sh also clears this per boot, but the image should not
# carry it in the first place.
defaults write com.apple.loginwindow TALLogoutSavesState -bool false 2>/dev/null
defaults write -g NSQuitAlwaysKeepsWindows -bool false 2>/dev/null
# Apple's spelling of the file, not a typo here.
gone "$HOME/Library/Group Containers/group.com.apple.loginwindow.persistent-apps/persistantApps"
gone "$HOME/Library/Saved Application State"
for app in "System Settings" "System Preferences"; do
  /usr/bin/pkill -x "$app" 2>/dev/null && echo "  quit $app"
done

say "The agent MCP registry (regenerated per run; the baked one is stale by design)"
# ~/.cua/agent-mcp.json is written by write-agent-mcp.sh, which run-agent.sh
# invokes before every agent run. Whatever the BUILD left behind is a snapshot of
# an older wiring, and it ships in the image.
#
# That matters more than it looks. An MCP client spawns every stdio server in its
# registry at AGENT STARTUP, not on first use, so a registry entry pointing
# straight at an app-backed MCP runner is an app launched in every Space
# regardless of what was asked -- the `c59803a6f` bug class. Measured on this
# image: the baked file still had
#   "blender": { "command": "uvx", "args": ["blender-mcp"] }
# the pre-hot-load wiring, months of fixes out of date, sitting in front of
# mcp-lazy-app.py. Anything that read it before write-agent-mcp.sh ran got the
# old behaviour back.
#
# Deleting it removes the whole class: the only registry that can exist in a
# Space is the one the generator just wrote.
gone "$HOME/.cua/agent-mcp.json"

say "Logs and transient per-boot state"
gone "$HOME/Library/Logs/Unity"
# The spacesd token (whoever Init'ed the build VM) must never ship: every
# clone boots in bootstrap mode and gets its own token from its first client.
gone "$HOME/.cua/spacesd/token"
gone "$HOME/.cua/env-token"
gone "$HOME/.rcdp-token"
gone "$HOME/.rcdp-handoff-token"
rm -f "$HOME"/.cua/*.log "$HOME"/.cua/*.jsonl "$HOME"/.cua/*.err 2>/dev/null
# The permission approver's "this Space is blocked by a credential panel" marker.
# If the BUILD ever tripped a panel, this file ships in the image and then every
# clone asserts it is blocked, from its first second, forever -- a marker that
# tells an agent to stop waiting on a Space that is fine. prepare-space.sh also
# clears it per boot; the image must not carry one in the first place.
gone "$HOME/.cua/SPACE-BLOCKED-credential-panel.txt"
# The hot-load shim's per-app state: launch logs and flock files. All of it is
# per-boot (write-agent-mcp.sh rewrites the specs on every run) and a stale lock
# file shipping in the image is pure noise -- flock ownership does not survive a
# reboot, but the file's mtime would be the builder's.
rm -rf "$HOME"/.cua/lazy-apps 2>/dev/null
rm -f "$HOME"/.cua/shot-*.png "$HOME"/.cua/rshot-*.png "$HOME"/.cua/*.png 2>/dev/null

# The service logs too. Every com.trycua LaunchAgent plist uses StandardOutPath
# / StandardErrorPath, which APPEND, so whatever the build left in them ships in
# the image and every clone inherits it and keeps appending underneath.
#
# This is not tidiness. It cost two investigations. The golden shipped a 23 KB
# com.trycua.cua-driver-local.out.log ending in 150 consecutive
#   [cua-driver] still waiting on: Accessibility, Screen Recording
# lines left over from the build. On a clone that file reads as a driver that is
# gated right now — it is not; its mtime never moves — and on a clone that IS
# gated the new lines are indistinguishable from the inherited ones. Both a
# failing clone and a healthy one presented an identical tail. Truncate, so the
# log a clone shows is that clone's own boot and nothing else.
for l in "$HOME"/.cua-server/com.trycua.*.log; do [ -f "$l" ] && : > "$l"; done
echo "  truncated com.trycua service logs (they append across boots and clones)"

say "Verifying nothing identity-bearing remains"
fail=0
check_absent() { [ -e "$1" ] && { echo "  STILL PRESENT: $1"; fail=1; } || echo "  clean: $1"; }
check_absent "$HOME/Library/Unity/licenses"
check_absent "$HOME/Library/Application Support/UnityHub/accounts.db"
check_absent "$HOME/.plastic4"
check_absent "$HOME/.claude.json"
check_absent "$HOME/.claude/.credentials.json"
check_absent "$HOME/.claude/projects"
# The other half of the same contract: the skills MUST still be there. An image
# that sanitises them away is how the last run's agent ended up with no Blender
# skill at all.
for s in "$HOME"/.claude/skills/*/SKILL.md; do
  [ -f "$s" ] && echo "  present: $s" || { echo "  MISSING: agent skills"; fail=1; }
done
check_absent "$HOME/biome-tiles"
check_absent "$HOME/Library/Application Support/Google/Chrome"
if security find-generic-password -s unity >/dev/null 2>&1; then
  echo "  STILL PRESENT: keychain 'unity'"; fail=1
else
  echo "  clean: keychain 'unity'"
fi
# The invariant the whole keychain fix rests on: the shipped login keychain is
# the default, is on the search list, and takes the account password. Checked
# with an operation that genuinely verifies the password — one taking `-k`.
# `unlock-keychain` is useless as a test here (it succeeds on an already
# unlocked keychain without looking at the password) and believing it is what
# sent two earlier investigations down the wrong path.
LOGIN_KC="$HOME/Library/Keychains/login.keychain-db"
# /etc/kcpassword must decode to exactly the account password. If it does not,
# loginwindow cannot unlock the login keychain at autologin and replaces it with
# an AKS-keyed one whose password nobody knows — see the repair step above.
if sudo_run /usr/bin/python3 -c '
import sys
key = bytes.fromhex("7d895223d2bcddeaa3b91f")
raw = open("/etc/kcpassword", "rb").read()
out = bytearray()
for i, b in enumerate(raw):
    c = b ^ key[i % len(key)]
    if c == 0:
        break
    out.append(c)
sys.exit(0 if bytes(out) == b"lume" else 1)
' 2>/dev/null; then
  echo "  clean: /etc/kcpassword decodes to the account password"
else
  echo "  WRONG: /etc/kcpassword does not decode to the account password — every"
  echo "         boot will replace the login keychain with an unopenable one"; fail=1
fi
if security default-keychain -d user 2>/dev/null | grep -q "login.keychain-db"; then
  echo "  clean: login is the shipped DEFAULT keychain"
else
  echo "  WRONG: the shipped default keychain is not login"; fail=1
fi
if security add-generic-password -U -s cua-sanitize-probe -a probe -w x -A "$LOGIN_KC" 2>/dev/null \
   && security set-generic-password-partition-list \
        -S "teamid:0000000000,apple:,apple-tool:" -k lume \
        -s cua-sanitize-probe -a probe "$LOGIN_KC" >/dev/null 2>&1; then
  echo "  clean: login keychain takes the account password for ACL writes"
else
  echo "  WRONG: login keychain does NOT take the account password — every"
  echo "         teleport into a clone will raise an unanswerable dialog"; fail=1
fi
security delete-generic-password -s cua-sanitize-probe -a probe "$LOGIN_KC" >/dev/null 2>&1
# No trace of the old `cua` keychain workaround may ship.
if [ -e "$HOME/Library/Keychains/cua.keychain-db" ]; then
  echo "  STILL PRESENT: cua.keychain-db is baked into the image"; fail=1
else
  echo "  clean: no cua.keychain-db in the image"
fi
if [ -e "$HOME/Library/LaunchAgents/com.trycua.keychain-unlock.plist" ]; then
  echo "  STILL PRESENT: the legacy keychain-unlock watchdog LaunchAgent"; fail=1
else
  echo "  clean: no keychain-unlock watchdog in the image"
fi
stale=$(ls -1 "$HOME/Library/Keychains/"login_renamed_*.keychain-db 2>/dev/null | wc -l | tr -d ' ')
if [ "${stale:-0}" -gt 0 ]; then
  echo "  STILL PRESENT: $stale stale login_renamed_N keychain(s)"; fail=1
else
  echo "  clean: no stale login_renamed_N keychains"
fi
# Nothing in the image may reference a home directory other than the guest's
# own (/Users/lume) — that would be a path leaked from the building machine.
# Exclude this script, whose own pattern would otherwise match.
leaked=$(grep -rIl -e "/Users/" "$HOME/.cua" 2>/dev/null \
  | grep -v "sanitize-golden.sh" \
  | while read -r f; do
      if grep -oI "/Users/[A-Za-z0-9._-]*" "$f" 2>/dev/null | sort -u | grep -qv "^/Users/lume$"; then
        echo "$f"
      fi
    done)
if [ -n "$leaked" ]; then
  echo "  WARNING: a non-guest home path is referenced under ~/.cua:"
  printf '%s\n' "$leaked" | sed 's/^/    /'
  fail=1
else
  echo "  clean: no foreign /Users/<name> paths under ~/.cua"
fi

# Scan for credentials or account identifiers left in scripts. These are the
# shapes that actually leaked before: a baked OAuth token, the Hub's Safe
# Storage key, and a Unity numeric account id.
secret_hits=$(grep -rIn -e "accessToken" -e "Safe Storage\" -a" -e "auth-tokens:[0-9]" \
  "$HOME/.cua" 2>/dev/null | grep -v "sanitize-golden.sh" | head -5)
if [ -n "$secret_hits" ]; then
  echo "  WARNING: credential-shaped content still under ~/.cua:"
  printf '%s\n' "$secret_hits" | sed 's/^/    /'
  fail=1
else
  echo "  clean: no credential-shaped content under ~/.cua"
fi
scaffold=$(ls "$HOME"/.cua/make_hoverboard.py "$HOME"/.cua/mcp_probe*.py "$HOME"/.cua/demo-out.txt 2>/dev/null)
if [ -n "$scaffold" ]; then
  echo "  STILL PRESENT: demo scaffolding:"; printf '%s\n' "$scaffold" | sed 's/^/    /'; fail=1
else
  echo "  clean: no demo scaffolding"
fi
if [ -f "$HOME/Library/LaunchAgents/com.trycua.prepare-demo.plist" ]; then
  echo "  STILL PRESENT: token-installing boot agent"; fail=1
else
  echo "  clean: no token-installing boot agent"
fi

# MiniBuddy. `MiniBuddyLaunch` in the user's com.apple.loginwindow domain is the
# only thing loginwindow consults before launching the post-update Setup
# Assistant, and it keys off the key's EXISTENCE, not its value: with the key
# present the log says "MiniBuddyLaunch pref is set" and Setup Assistant owns
# the session, whether the value is 1 or 0. So the key must be ABSENT.
#
# Read the plist, not `defaults read`: while Setup Assistant is running it holds
# the domain in the user's cfprefsd and `defaults read` answers from that cache,
# which is how an image once shipped armed with every check reporting clean.
LWP="$HOME/Library/Preferences/com.apple.loginwindow.plist"
mb="$(plutil -extract MiniBuddyLaunch raw -o - "$LWP" 2>/dev/null)"
byhost="$(ls -1 "$HOME"/Library/Preferences/ByHost/com.apple.loginwindow.*.plist 2>/dev/null | wc -l | tr -d ' ')"
sa="$(pgrep -f '/Setup Assistant.app/Contents/MacOS/Setup Assistant' | wc -l | tr -d ' ')"
if [ -n "$mb" ] || [ "${byhost:-0}" != "0" ] || [ "${sa:-0}" != "0" ]; then
  echo "  STILL ARMED: MiniBuddyLaunch=${mb:-absent}, ${byhost:-0} ByHost plist(s), ${sa:-0} Setup Assistant running"
  echo "    run suppress-setup-assistant.sh, reboot the guest, run it again"
  fail=1
else
  echo "  clean: MiniBuddy disarmed (MiniBuddyLaunch absent, no ByHost loginwindow prefs)"
fi

if [ "$fail" -ne 0 ]; then
  echo; echo "SANITIZE INCOMPLETE — do not publish this image." >&2; exit 1
fi
echo; say "Image is clean. Identity now arrives only via teleport."
