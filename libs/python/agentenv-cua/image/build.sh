#!/bin/bash
# Build the AgentEnv macOS bench image with lume, on an Apple-silicon Mac:
#
#   clone the base -> boot with this folder shared read-only -> provision.sh in the guest -> reboot ->
#   check the driver's card answers -> shut down -> [lume push]
#
#   image/build.sh --base-vm NAME | --base-image ghcr.io/trycua/macos:26-<pin>
#                  [--name VM] [--push NAME:TAG]
#
# The built VM is kept under --name (the lume provider's `base`); a failed build is deleted.
# The base is the public ghcr.io/trycua/macos:26 image (full tier: autologin user lume, SIP off, ssh, uv). Pin its
# dated tag, not the floating one. Apple's licence allows two macOS VMs per Mac: this refuses to start a third.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PKG="$(cd "$HERE/.." && pwd)"

base_vm="" base_image="" name="agentenv-macos-bench" push="" built=0
while [ $# -gt 0 ]; do
  case "$1" in
    --base-vm) base_vm=$2; shift 2 ;;
    --base-image) base_image=$2; shift 2 ;;
    --name) name=$2; shift 2 ;;
    --push) push=$2; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
[ -n "$base_vm$base_image" ] || { echo "give --base-vm or --base-image" >&2; exit 2; }

# Count by process, not by `lume ls` status: a VM another lume instance runs can show as stopped there.
running_vms() { ps -axo command= | awk '$1 ~ /(^|\/)lume$/ && $2=="run" || $1 ~ /(^|\/)lume$/ && $3=="run"' | wc -l | tr -d ' '; }
require_slot() {
  local running; running=$(running_vms)
  [ "$running" -lt 2 ] || { echo "$running macOS VMs already run on this Mac; not starting a third" >&2; exit 3; }
}
require_slot

if [ -n "$base_image" ]; then
  registry=${base_image%%/*}; rest=${base_image#*/}; org=${rest%%/*}; repo=${rest#*/}
  lume pull "$repo" "$name" --registry "$registry" --organization "$org"
else
  lume clone "$base_vm" "$name"
fi
lume set "$name" --display 1280x800

# lume mounts a shared folder at /Volumes/My Shared Files/<its basename>, so the folder is named `payload`.
payload_root=$(mktemp -d)
payload="$payload_root/payload"
mkdir -p "$payload"
cp "$HERE/provision.sh" "$HERE/set-display.swift" "$payload/"
cp -R "$PKG/src/agentenv_cua/drivers/cua_driver_macos" "$payload/"
cleanup() { if [ "$built" = 0 ]; then lume stop "$name" >/dev/null 2>&1 || true; lume delete "$name" --force >/dev/null 2>&1 || true; fi; rm -rf "$payload_root"; }
trap cleanup EXIT

require_slot
lume run "$name" --display none --detach --shared-dir "$payload:ro"
for _ in $(seq 1 120); do lume ssh "$name" true --timeout 10 >/dev/null 2>&1 && break; sleep 5; done
lume ssh "$name" 'bash "/Volumes/My Shared Files/payload/provision.sh" "/Volumes/My Shared Files/payload"' --timeout 3600

# A cold boot, as a provider's clone boots: sync first, since a stop drops unsynced writes.
lume ssh "$name" 'sync' --timeout 30 || true
lume stop "$name"
require_slot
lume run "$name" --display none --detach --shared-dir "$payload:ro"
for _ in $(seq 1 120); do lume ssh "$name" true --timeout 10 >/dev/null 2>&1 && break; sleep 5; done
# The tool manifest is pinned per OS. With none in the payload, pin it from the running daemon now and write it
# back beside the driver source, to review and commit.
manifest="$PKG/src/agentenv_cua/drivers/cua_driver_macos/mcp.json"
if [ ! -f "$manifest" ]; then
  sleep 20  # the daemon's LaunchAgent starts at login
  lume ssh "$name" 'cd /opt/agentenv/drivers/cua-driver && venv/bin/python pin_manifest.py >&2 && cat mcp.json' \
    --timeout 300 > "$manifest.new"
  python3 -I -c 'import json,sys; json.load(open(sys.argv[1]))' "$manifest.new"
  mv "$manifest.new" "$manifest"
  lume ssh "$name" 'launchctl kickstart -k gui/$(id -u)/com.trycua.agentenv.driver' --timeout 30
  echo "pinned $manifest; review and commit it"
fi
ip=$(lume get "$name" --format json | python3 -I -c 'import json,sys; d=json.load(sys.stdin); d=d[0] if isinstance(d,list) else d; print(d.get("ipAddress") or "")')
ok=0
for _ in $(seq 1 60); do curl -fsS "http://$ip:18765/.well-known/agent-env.json" >/dev/null 2>&1 && { ok=1; break; }; sleep 5; done
[ "$ok" = 1 ] || { echo "the driver's card didn't answer at http://$ip:18765 after the reboot" >&2; exit 1; }
echo "driver card answers at http://$ip:18765"

lume ssh "$name" 'sync' --timeout 30 || true
lume stop "$name"
built=1
if [ -n "$push" ]; then
  lume push "$name" "$push"
fi
echo "built $name"
