#!/bin/bash
# Add the other bench arms and the bench's macOS apps to a bench image built by build.sh, with lume:
#
#   clone the built image -> boot with this package's drivers and apps shared read-only -> provision-arms.sh in the
#   guest -> sync -> stop -> [lume push]
#
#   image/build-arms.sh --base-vm agentenv-macos-bench [--name agentenv-macos-bench-arms] [--arms a,b] [--push NAME:TAG]
#
# --arms defaults to agent-desktop,macos-harness. Add codex-cu only for an image that stays private: its arm needs a
# ChatGPT login inside the VM, which this doesn't do (provision-arms.sh says how). Apple's licence allows two macOS VMs
# per Mac: this refuses to start a third.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PKG="$(cd "$HERE/.." && pwd)"

base_vm="" name="agentenv-macos-bench-arms" arms="agent-desktop,macos-harness" push="" built=0
while [ $# -gt 0 ]; do
  case "$1" in
    --base-vm) base_vm=$2; shift 2 ;;
    --name) name=$2; shift 2 ;;
    --arms) arms=$2; shift 2 ;;
    --push) push=$2; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
[ -n "$base_vm" ] || { echo "give --base-vm" >&2; exit 2; }

running=$(lume ls --format json | python3 -I -c 'import json,sys; print(sum(v.get("status")=="running" for v in json.load(sys.stdin)))')
[ "$running" -lt 2 ] || { echo "$running macOS VMs already run on this Mac; not starting a third" >&2; exit 3; }

lume clone "$base_vm" "$name"
payload=$(mktemp -d)
mkdir -p "$payload/payload"
cp "$HERE/provision-arms.sh" "$HERE/install_app.py" "$payload/payload/"
cp -R "$PKG/src/agentenv_cua/macos-drivers" "$PKG/src/agentenv_cua/macos-apps" "$payload/payload/"
cleanup() { if [ "$built" = 0 ]; then lume stop "$name" >/dev/null 2>&1 || true; lume delete "$name" --force >/dev/null 2>&1 || true; fi; rm -rf "$payload"; }
trap cleanup EXIT

lume run "$name" --display none --detach --shared-dir "$payload:ro"
for _ in $(seq 1 120); do lume ssh "$name" true --timeout 10 >/dev/null 2>&1 && break; sleep 5; done
lume ssh "$name" "bash '/Volumes/My Shared Files/payload/provision-arms.sh' '/Volumes/My Shared Files/payload' --arms $arms" --timeout 7200
# A stop drops unsynced writes: sync first.
lume ssh "$name" 'sync' --timeout 30 || true
lume stop "$name"
built=1
[ -z "$push" ] || lume push "$name" "$push"
echo "built $name"
