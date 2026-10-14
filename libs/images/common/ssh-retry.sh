# shellcheck shell=bash
# Sourced by boot-qemu.sh (and its test): retry an ssh command until a deadline.
#
#   ssh_retry DEADLINE CONNECT_TIMEOUT PAUSE -- SSH_ARGS...
#
# Runs `${SSH_BIN:-ssh} -o ConnectTimeout=CONNECT_TIMEOUT SSH_ARGS...` until it
# succeeds or SECONDS reaches DEADLINE (an absolute $SECONDS value). Only
# connection failures (ssh exit 255: refused, reset, "timed out during banner
# exchange" while a TCG guest's sshd is still starting) are retried; any other
# status is the remote command's and returns at once. Each attempt is logged.
# Returns the last status.
ssh_retry() {
    local deadline="$1" connect="$2" pause="$3" attempt=0 rc=255 start=$SECONDS
    shift 3
    [ "${1:-}" = -- ] && shift
    while :; do
        attempt=$((attempt + 1))
        rc=0
        "${SSH_BIN:-ssh}" -o ConnectTimeout="$connect" "$@" || rc=$?
        if [ "$rc" != 255 ]; then
            echo "  ssh attempt $attempt: exit $rc after $((SECONDS - start))s" >&2
            return "$rc"
        fi
        if [ $((SECONDS + pause)) -ge "$deadline" ]; then
            echo "  ssh attempt $attempt: connection failed (255); budget spent after $((SECONDS - start))s" >&2
            return "$rc"
        fi
        echo "  ssh attempt $attempt: connection failed (255), retrying in ${pause}s ($((deadline - SECONDS))s left)" >&2
        sleep "$pause"
    done
}
