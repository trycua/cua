#!/usr/bin/env bash
# Smoke test of the docker tier's VM disk (ghcr.io/trycua/linux:24.04-docker-disk):
# boot it under QEMU (common/boot-qemu.sh: a throwaway overlay, KVM/HVF when
# the host has it, else TCG), then run tests/docker-vm-guest.sh in the guest
# as root over ssh: dockerd up, overlay + br_netfilter loaded at boot,
# iptables/ip6tables (nat REDIRECT, physdev), a compose hello-world.
#
#   smoke-docker-vm.sh --disk DISK [--arch amd64|arm64] [--evidence DIR] [--timeout S]
#
# The guest's `RESULT {json}` line (dockerd-ready seconds after boot, compose
# timings) lands in DIR/docker-vm.json; its output in DIR/docker-vm.log.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
host_arch() { case "$(uname -m)" in arm64|aarch64) echo arm64 ;; *) echo amd64 ;; esac; }
DISK=""; ARCH="$(host_arch)"; EVIDENCE=""; TIMEOUT=""
while [ $# -gt 0 ]; do
    case "$1" in
        --disk) DISK="$2"; shift 2 ;;
        --arch) ARCH="$2"; shift 2 ;;
        --evidence) EVIDENCE="$2"; shift 2 ;;
        --timeout) TIMEOUT="$2"; shift 2 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
[ -f "$DISK" ] || { echo "--disk: no such file: $DISK" >&2; exit 2; }
EVIDENCE="${EVIDENCE:-$(mktemp -d)}"
mkdir -p "$EVIDENCE"
EVIDENCE="$(cd "$EVIDENCE" && pwd)"
# TCG (no KVM/HVF, e.g. hosted arm64 runners, or a foreign arch) is ~10x slower.
accel_ok=0
case "$(uname -s)" in
    Darwin) [ "$ARCH" = "$(host_arch)" ] && accel_ok=1 ;;
    Linux) [ -w /dev/kvm ] && [ "$ARCH" = "$(host_arch)" ] && accel_ok=1 ;;
esac
[ -n "$TIMEOUT" ] || { [ "$accel_ok" = 1 ] && TIMEOUT=300 || TIMEOUT=1500; }
export CUA_SMOKE_GUEST="$HERE/tests/docker-vm-guest.sh" CUA_SMOKE_EVIDENCE="$EVIDENCE"
# Runs on the host once the guest's ssh answers: pipe the guest script in
# (every wait in it is bounded; keepalives end a hung connection).
run='ssh -i "$QEMU_SSH_KEY" -p "$QEMU_FWD_22" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
  -o BatchMode=yes -o ConnectTimeout=30 -o ServerAliveInterval=15 -o ServerAliveCountMax=8 cua@127.0.0.1 "sudo bash -s" \
  <"$CUA_SMOKE_GUEST" | tee "$CUA_SMOKE_EVIDENCE/docker-vm.log"; rc=${PIPESTATUS[0]}
  grep "^RESULT " "$CUA_SMOKE_EVIDENCE/docker-vm.log" | sed "s/^RESULT //" >"$CUA_SMOKE_EVIDENCE/docker-vm.json" || true
  exit "$rc"'
"$HERE/../common/boot-qemu.sh" "$DISK" --arch "$ARCH" --mem 4G --fwd 22 --ssh-cmd true \
    --timeout "$TIMEOUT" --log "$EVIDENCE/docker-vm-serial.log" --run "$run"
echo "docker tier smoke: $(cat "$EVIDENCE/docker-vm.json")"
