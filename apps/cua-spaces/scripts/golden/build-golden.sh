#!/bin/bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# build-golden.sh — build the macOS Spaces golden image from a base VM.
#
# Run this ON THE HOST. It drives a VM that already has macOS installed and is
# reachable over `lume ssh`, copies this directory in, and runs each step in
# order inside the guest.
#
#   ./build-golden.sh [vm-name]        # default: cua-golden
#
# Prerequisites on the host:
#   - lume, with the VM created and running
#   - the signed daemon bundles (the guest has no Rust toolchain):
#       CuaDriverLocal.app  (libs/cua-driver)
#       Cua Spacesd.app  (libs/cua-spacesd/scripts/build-macos-app.sh,
#                            bundle id com.trycua.cua-env-driver)
#     in $BUNDLE_DIR (default /Applications).
#
# The last step, sanitize-golden.sh, strips every trace of THIS machine's
# identity and exits non-zero if anything survives. It must run last, and the
# image must not be published if it fails.
set -uo pipefail

VM="${1:-cua-golden}"
SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# Signed .app bundles for the daemons, including "Cua Spacesd.app"
# (see install-agent-stack.sh).
BUNDLE_DIR="${BUNDLE_DIR:-/Applications}"
# Per-step ceiling. The Unity Editor download alone can run well past 20 min.
STEP_TIMEOUT="${STEP_TIMEOUT:-3600}"
REMOTE_SRC="/Users/lume/golden-src"

say()  { printf '\n\033[1;36m==> %s\033[0m\n' "$*"; }
fail() { printf '\033[1;31m!! %s\033[0m\n' "$*" >&2; exit 1; }

ip_of() {
  curl -s -m 8 "http://127.0.0.1:7777/lume/vms/$VM" \
    | /usr/bin/python3 -c 'import json,sys; print(json.load(sys.stdin).get("ipAddress") or "")' 2>/dev/null
}

IP="$(ip_of)"
[ -n "$IP" ] || fail "$VM is not running (or lume serve is down). Start it, then retry."
say "Building $VM at $IP"

# --- copy this directory + the prebuilt binaries into the guest -------------
say "Staging sources"
lume ssh "$VM" "mkdir -p $REMOTE_SRC ~/.cua/stage" >/dev/null 2>&1
# scp is used rather than `lume ssh 'cat >'` so binaries survive intact.
sshpass -p lume scp -q -r -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
  "$SRC"/. "lume@$IP:$REMOTE_SRC/" || fail "failed to copy sources"
# The daemons must be their SIGNED .app bundles, because macOS grants screen
# recording and accessibility to a bundle identity and the golden's TCC seeds
# target those identities.
for app in "CuaDriverLocal.app" "Cua Spacesd.app"; do
  src="$BUNDLE_DIR/$app"
  [ -d "$src" ] || fail "missing $src — point BUNDLE_DIR at the built .app bundles"
  sshpass -p lume scp -q -r -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
    "$src" "lume@$IP:/Users/lume/.cua/stage/" || fail "failed to copy $app"
done

# Run a build step in the guest. Steps here run for MINUTES — the Command Line
# Tools and the Unity Editor are each large downloads — and `lume ssh` gives up
# long before they finish ("SSH operation timed out"), killing a step that was
# progressing fine. So launch it detached, writing its output and a final exit
# code to files, then poll with short-lived SSH calls that cannot time out.
step() {  # description, remote command
  local desc="$1" cmd="$2"
  local tag; tag="$(echo "$desc" | tr -cd '[:alnum:]' | cut -c1-24)"
  # Keep step logs OUT of ~/.cua: the sanitizer wipes ~/.cua/*.log, which would
  # delete its own failure report before anyone could read it.
  local log="/tmp/cua-build-${tag}.log" rc="/tmp/cua-build-${tag}.rc"
  say "$desc"

  lume ssh "$VM" "rm -f '$log' '$rc'; nohup /bin/bash -lc '{ $cmd; } >\"$log\" 2>&1; echo \$? >\"$rc\"' >/dev/null 2>&1 & echo started" >/dev/null 2>&1 \
    || fail "could not start: $desc"

  local waited=0 shown=0
  while :; do
    sleep 15; waited=$((waited + 15))
    # Show new output as it appears, so a long step is visibly alive.
    local lines; lines="$(lume ssh "$VM" "wc -l < '$log' 2>/dev/null || echo 0" 2>/dev/null | tr -d ' \r\n')"
    if [ -n "${lines:-}" ] && [ "${lines:-0}" -gt "$shown" ] 2>/dev/null; then
      lume ssh "$VM" "tail -n +$((shown + 1)) '$log' 2>/dev/null | tail -8" 2>/dev/null | sed 's/^/    /'
      shown="$lines"
    fi
    local code; code="$(lume ssh "$VM" "cat '$rc' 2>/dev/null" 2>/dev/null | tr -d ' \r\n')"
    if [ -n "${code:-}" ]; then
      [ "$code" = "0" ] || {
        lume ssh "$VM" "tail -25 '$log'" 2>/dev/null | sed 's/^/    /'
        fail "step failed (exit $code): $desc"
      }
      printf '    (%ds)\n' "$waited"
      return 0
    fi
    [ "$waited" -ge "$STEP_TIMEOUT" ] && fail "step timed out after ${waited}s: $desc"
  done
}

# --- build steps, in order --------------------------------------------------
# CLT first: Unity's package manager shells out to git, and the git *stub*
# raises a GUI dialog that nothing in a Space can dismiss.
step "Command Line Tools"        "bash $REMOTE_SRC/install-clt.sh"
step "Unity Hub + Editor"        "bash $REMOTE_SRC/install-unity.sh"
step "Blender + blender-mcp"     "bash $REMOTE_SRC/blender-setup.sh"
# Chrome is a teleport target, not a demo prop: the spacesd's Chrome import relaunches
# /Applications/Google Chrome.app in the destination, so without this step a
# "teleport my chrome" lands a bundle and has nothing to launch.
step "Google Chrome"             "bash $REMOTE_SRC/install-chrome.sh"
step "Agent stack (cua-driver, cua-spacesd)" \
                                 "STAGE=/Users/lume/.cua/stage bash $REMOTE_SRC/install-agent-stack.sh"
step "get-skills MCP + skills"   "bash $REMOTE_SRC/install-skills.sh"
# The agent CLI itself. Without it the image is complete right up until the
# moment you try a run, which then dies with "exec: claude: not found".
step "Coding-agent CLI"          "bash $REMOTE_SRC/install-agent-cli.sh"

# Fixes that make an unattended agent run survive at all.
step "TCC pre-grants"            "bash $REMOTE_SRC/seed-tcc.sh"
step "Python 3.10+ for MCP for Unity" "bash $REMOTE_SRC/seed-python-for-unity-mcp.sh"
# The official Unity CLI: `unity mcp` is the image's Unity MCP server. The Hub
# installs the binary on its first run; this puts it on the GUI PATH and proves
# the server starts.
step "Unity CLI on the GUI PATH"  "bash $REMOTE_SRC/install-unity-cli.sh"

# Per-boot preparation + the agent launcher live in the image. seed-tcc.sh goes
# with them: prepare-space.sh re-runs it as `--refresh` as its FIRST action on
# every boot, to rewrite the ReplayKit screen-capture approval ledger before
# cua-spacesd's first capture can raise the "bypass the private window picker" alert.
# blender-mcp-start.py goes with them too, and mcp-lazy-app.py with it: the shim
# is what launches Blender on demand and feeds it that starter (write-agent-mcp.sh
# writes the spec), now that nothing launches applications at boot.
step "Per-boot prepare + agent launcher" \
  "install -m 755 $REMOTE_SRC/prepare-space.sh $REMOTE_SRC/write-agent-mcp.sh $REMOTE_SRC/run-agent.sh $REMOTE_SRC/seed-tcc.sh $REMOTE_SRC/unity-hub-markers.sh $REMOTE_SRC/permission-approver.py ~/.cua/ && install -m 644 $REMOTE_SRC/blender-mcp-start.py ~/.cua/ && install -m 755 $REMOTE_SRC/mcp-lazy-app.py ~/.cua/ && echo installed"

# Disarm MiniBuddy. Must come after every step that can make macOS update
# itself, and before sanitize. Without it every clone boots into the post-update
# Setup Assistant and owns the session, so the Space can never start.
#
# TWO passes with a reboot between them, and the reboot is not optional. On the
# first pass MiniBuddy is still running and holds com.apple.loginwindow open in
# the user's cfprefsd; whatever the build writes is flushed away by that stale
# copy when the VM stops, and the image ships re-armed. The first pass therefore
# exits 2 ("not yet certified"). After the reboot MiniBuddy no longer launches,
# so the second pass can verify the plist on disk and pass.
say "Disarming MiniBuddy (pass 1)"
lume ssh "$VM" "bash $REMOTE_SRC/suppress-setup-assistant.sh; true" 2>&1 | sed 's/^/    /'

say "Rebooting the guest so nothing holds com.apple.loginwindow open"
lume ssh "$VM" "printf '%s\n' \"\${CUA_SUDO_PW:-lume}\" | sudo -S -p '' shutdown -r now" >/dev/null 2>&1 || true
sleep 30
for _ in $(seq 1 60); do
  lume ssh "$VM" 'echo up' >/dev/null 2>&1 && break
  sleep 5
done

step "Suppress Setup Assistant (MiniBuddy)" \
                                 "bash $REMOTE_SRC/suppress-setup-assistant.sh"

# Remove build inputs BEFORE sanitising, so what the sanitizer verifies is
# exactly what ships. (Staged sources carry the builder's paths.)
say "Removing build sources from the image"
# verify-golden.sh + its manifest go with the sanitizer: both are publish gates
# that must outlive the staged sources, and the manifest is the image's own
# record of what it is supposed to contain.
lume ssh "$VM" "install -m 755 $REMOTE_SRC/sanitize-golden.sh $REMOTE_SRC/verify-golden.sh ~/.cua/ && install -m 644 $REMOTE_SRC/golden-required.txt ~/.cua/ && rm -rf $REMOTE_SRC ~/.cua/stage && rm -f /tmp/cua-build-*.log /tmp/cua-build-*.rc" >/dev/null 2>&1

# MUST be last: strips identity, and fails the build if any remains.
step "Sanitize (identity + scaffolding)" "~/.cua/sanitize-golden.sh"

# The content gate. Runs AFTER sanitize so what it certifies is exactly what
# ships, and fails the build rather than letting a thinner-than-expected image
# reach a publish. See check-golden-coverage.sh for why this backstop exists.
step "Verify golden contents" "~/.cua/verify-golden.sh ~/.cua/golden-required.txt"

say "Freezing"
curl -s -X POST "http://127.0.0.1:7777/lume/vms/$VM/stop" -m 60 >/dev/null 2>&1
for _ in $(seq 1 10); do
  sleep 6
  [ -z "$(ip_of)" ] && break
done
# The API stop does NOT win against a VM someone started with the `lume run`
# CLI (as you must, to reach recoveryOS and disable SIP): that process owns the
# machine and keeps it up. Everything after this point — `lume set`, and the
# "built and frozen" banner — is then a lie, and the build used to exit 0
# anyway. Say so instead.
if [ -n "$(ip_of)" ]; then
  fail "$VM did not stop. A foreground \`lume run $VM\` still owns it — kill that
process, then finish by hand:
  curl -s -X POST http://127.0.0.1:7777/lume/vms/$VM/stop
  lume set $VM --cpu 8"
fi

# Enforce the compute spec rather than trusting the `lume create` line a builder
# typed. Unity's asset importer is parallel, so vCPU count is the dominant term
# in first-import wall-clock; a golden left at the 4 vCPU default imports the
# demo project in roughly twice the time. Memory is deliberately NOT raised:
# on an 18 GB host a larger guest pushes the *host* into swap and the import
# gets slower, so this is the one variable worth moving.
echo "==> Enforcing compute spec (8 vCPU) on $VM"
lume set "$VM" --cpu 8 || fail "could not set the compute spec on $VM"

cat <<EOF

============================================================================
 $VM built and frozen.
============================================================================
Before first use, give this installation its own machine identity so your
Spaces are not the same machine as every other builder's:

  lume set $VM --machine-identifier random

Then clone, teleport, and run.
============================================================================
EOF
