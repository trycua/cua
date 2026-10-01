#!/usr/bin/env bash
# Sourced by the spacesd scripts: decides whether this boot uses Fleet's
# per-claim token file (await-token-file mode) or a local token.
#
# Fleet delivers the claim's env token as a Secret mounted read-only at
# /run/cua (gVisor/macOS pods: a Secret volume; KubeVirt: virtiofs tag
# cua-claim-secrets, mounted by run-cua.mount). The file /run/cua/env-token is
# root-owned 0600, empty until a claim binds, rewritten on rotation and
# emptied on release.
#
# Await mode when, in order:
#   CUA_ENV_AWAIT_TOKEN_FILE=1        forced on (=0 forces it off)
#   CUA_ENV_TOKEN or /etc/cua/env-token is set   -> off (local docker / QEMU)
#   /run/cua is a mount point         -> on (Fleet claim secrets)

# CUA_SPACESD_* is the new spelling of every CUA_ENV_* variable. The old
# names keep working, as does the interim CUA_GUESTD_* spelling; precedence
# is CUA_SPACESD_*, then CUA_GUESTD_*, then CUA_ENV_*.
for _cua_prefix in CUA_GUESTD_ CUA_SPACESD_; do
    for _cua_var in $(compgen -e); do
        case "$_cua_var" in
            "$_cua_prefix"?*) export "CUA_ENV_${_cua_var#"$_cua_prefix"}=${!_cua_var}" ;;
        esac
    done
done
unset _cua_var _cua_prefix

CUA_CLAIM_SECRETS_DIR="${CUA_CLAIM_SECRETS_DIR:-/run/cua}"
CUA_CLAIM_TOKEN_FILE="$CUA_CLAIM_SECRETS_DIR/env-token"
# Copy readable by the unprivileged driver, kept by `cua-spacesd token-sync`
# (root). Its directory is root-owned 0755 so the driver's user cannot swap it.
CUA_SYNCED_TOKEN_DIR="${CUA_SYNCED_TOKEN_DIR:-/run/cua-env}"
CUA_SYNCED_TOKEN_FILE="$CUA_SYNCED_TOKEN_DIR/env-token"

cua_is_mountpoint() {
    # mountinfo field 5 is the mount point; works under gVisor too.
    awk -v p="$1" '$5 == p { found = 1 } END { exit !found }' /proc/self/mountinfo 2>/dev/null
}

# KubeVirt: a virtio-fs device (virtio id 0x001a) means Fleet shares claim
# secrets. The udev rule pulls in run-cua.mount asynchronously, so a unit
# that checks the mount point during boot can run before it lands and fall
# back to a local token for good. Start the mount (idempotent, waits for it)
# before deciding. No-op without the device, without systemd, or in pods.
cua_wait_claim_mount() {
    cua_is_mountpoint "$CUA_CLAIM_SECRETS_DIR" && return 0
    grep -qs '^0x001a$' /sys/bus/virtio/devices/*/device || return 0
    command -v systemctl >/dev/null 2>&1 || return 0
    systemctl start run-cua.mount >/dev/null 2>&1 || true
}

cua_await_token_file() {
    case "${CUA_ENV_AWAIT_TOKEN_FILE:-}" in
        1|true|yes|on) return 0 ;;
        0|false|no|off) return 1 ;;
    esac
    [ -n "${CUA_ENV_TOKEN:-}" ] && return 1
    [ -s /etc/cua/env-token ] && return 1
    cua_wait_claim_mount
    cua_is_mountpoint "$CUA_CLAIM_SECRETS_DIR"
}
