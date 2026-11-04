#!/usr/bin/env bash
# Offline test for ssh-retry.sh (boot-qemu.sh's --ssh probe) with a fake ssh.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../ssh-retry.sh
. "$HERE/../ssh-retry.sh"
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
# Fake ssh: fails with 255 (banner timeout) FAILS times, then exits FINAL.
cat >"$tmp/ssh" <<'FAKE'
#!/usr/bin/env bash
n="$(cat "$STATE" 2>/dev/null || echo 0)"; echo $((n + 1)) >"$STATE"
echo "$*" >>"$STATE.args"
if [ "$n" -lt "$FAILS" ]; then echo "Connection timed out during banner exchange" >&2; exit 255; fi
exit "$FINAL"
FAKE
chmod +x "$tmp/ssh"
export SSH_BIN="$tmp/ssh" STATE="$tmp/count"
pass=0 fail=0
check() { # name want-rc want-attempts FAILS FINAL DEADLINE-OFFSET
    local name="$1" want="$2" attempts="$3" rc=0
    rm -f "$STATE" "$STATE.args"
    FAILS="$4" FINAL="$5" ssh_retry $((SECONDS + $6)) 7 0 -- -p 2222 cua@127.0.0.1 true 2>"$tmp/log" || rc=$?
    local got; got="$(cat "$STATE")"
    if [ "$rc" = "$want" ] && [ "$got" = "$attempts" ] && grep -q "ConnectTimeout=7" "$STATE.args"; then
        pass=$((pass + 1))
    else
        fail=$((fail + 1)); echo "FAIL $name: rc=$rc (want $want) attempts=$got (want $attempts)"; cat "$tmp/log"
    fi
}
check "first try"                 0 1 0 0 30
check "banner timeouts then ok"   0 4 3 0 30   # the arm64 TCG case: retried, then passes
check "command failure, no retry" 3 1 0 3 30   # the remote command's status is final
check "budget spent"              255 1 99 0 0 # no time left: one attempt, then give up
grep -q "attempt 1: connection failed" "$tmp/log" || { fail=$((fail + 1)); echo "FAIL attempts are not logged"; }
echo "pass=$pass fail=$fail"; [ "$fail" = 0 ]
