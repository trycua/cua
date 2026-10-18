#!/usr/bin/env bash
# Smoke test for the Fleet per-claim token path on the VM (containerDisk)
# variant under local QEMU: boots disk.img with a claim-secrets share
# (common/boot-qemu.sh --claim-secrets; 9p stands in for KubeVirt's virtiofs
# since macOS QEMU has no virtiofsd) and, from the host, writes, rotates and
# empties env-token in the shared directory while the guest's run-cua.mount,
# cua-env-token-sync and cua-spacesd (await-token-file mode) follow it.
# One VM, 4 GiB.
#
# Usage: smoke-claim-token-vm.sh [disk.img] [--arch arm64|amd64] [--timeout 600]
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
host_arch() { case "$(uname -m)" in arm64|aarch64) echo arm64 ;; *) echo amd64 ;; esac; }
ARCH="$(host_arch)"
DISK="${CUA_IMAGES_OUT:-$HOME/.cache/cua-images}/linux/$ARCH/disk.img"
TIMEOUT=600
while [ $# -gt 0 ]; do
    case "$1" in
        --arch) ARCH="$2"; shift 2 ;;
        --timeout) TIMEOUT="$2"; shift 2 ;;
        -*) echo "unknown option $1" >&2; exit 2 ;;
        *) DISK="$1"; shift ;;
    esac
done
CLAIM="$(mktemp -d "${TMPDIR:-/tmp}/cua-e2e-claim-vm.XXXXXX")"
trap 'rm -rf "$CLAIM"' EXIT
# The Secret file exists and is empty until a claim binds.
( umask 077; : >"$CLAIM/env-token" )

# Runs on the host once the guest's :3211 answers.
cat >"$CLAIM.run.sh" <<'RUN'
set -euo pipefail
PASS=0; FAIL=0
ok() { PASS=$((PASS + 1)); echo "  ok   $*"; }
bad() { FAIL=$((FAIL + 1)); echo "  FAIL $*"; }
URL="http://127.0.0.1:$QEMU_FWD_3211"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/cua-e2e-claim-vm-work.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
TOKEN_A="claimA-$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')"
TOKEN_B="claimB-$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')"
# Operator side: atomic rename in the shared directory, 0600.
set_token() { ( umask 077; printf '%s' "$1" >"$CLAIM/.env-token.tmp" && mv -f "$CLAIM/.env-token.tmp" "$CLAIM/env-token" ); }
clear_token() { ( umask 077; : >"$CLAIM/.env-token.tmp" && mv -f "$CLAIM/.env-token.tmp" "$CLAIM/env-token" ); }
. "$CLAIM_CHECKS" || exit 1
wait_driver 240 && ok "guest driver answers GetCapabilities" || { bad "guest driver never answered"; exit 1; }
echo "==> awaiting (no claim bound)"
claim_awaiting_checks
echo "==> claim binds: token A"
claim_install_checks
claim_rotate_revoke_checks
echo "==> VM claim-token checks: $PASS passed, $FAIL failed"
[ "$FAIL" = 0 ]
RUN
trap 'rm -rf "$CLAIM" "$CLAIM.run.sh"' EXIT
export CLAIM CLAIM_CHECKS="$HERE/claim-token-checks.sh"
"$HERE/../common/boot-qemu.sh" "$DISK" --arch "$ARCH" --mem 4G --smp 4 \
    --fwd 3211,22 --probe 3211 --claim-secrets "$CLAIM" --timeout "$TIMEOUT" \
    --ssh-cmd 'cat /sys/bus/virtio/devices/*/device | tr "\n" " "; echo; systemctl is-system-running; systemctl --no-pager --plain status run-cua.mount cua-env-token-sync.service cua-spacesd.service 2>&1 | grep -E "Loaded|Active|What|Where|^[a-z]" ; findmnt /run/cua; sudo -n ls -la /run/cua-env 2>/dev/null || ls -ld /run/cua-env' \
    --run "bash '$CLAIM.run.sh'"
