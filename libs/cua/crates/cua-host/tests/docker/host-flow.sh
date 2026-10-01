#!/usr/bin/env bash
# Runs INSIDE a throwaway Linux container: `cua host setup/status/stop/start/
# remove` against the fake relay directory, with a stub driver that records
# its command line. RUNNER=process (no init system) or RUNNER=systemd (the
# container runs systemd as PID 1; a system unit is installed as root).
set -euo pipefail
RUNNER="${RUNNER:-process}"
BIN=/opt/cua
fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "ok - $*"; }

export HOME=/root CUA_CREDENTIAL_STORE=file
rm -rf /root/.cua /tmp/stub-driver.args
mkdir -p /root/.cua

"$BIN/cua-fake-relay" acct-token user-1 ada@example.com >/tmp/relay.url 2>/tmp/relay.log &
RELAY_PID=$!
for _ in $(seq 1 50); do [ -s /tmp/relay.url ] && break; sleep 0.1; done
RELAY="$(head -n1 /tmp/relay.url)"
[ -n "$RELAY" ] || fail "fake relay did not start"
pass "fake relay at $RELAY"

# A `cua auth login` session in the file store (Linux default).
cat >/root/.cua/credentials.json <<JSON
{"access_token":"acct-token","refresh_token":null,"expires_at":"2099-01-01T00:00:00Z","token_type":"Bearer"}
JSON
chmod 600 /root/.cua/credentials.json

cua() { "$BIN/cua" "$@"; }

cua host setup --relay "$RELAY" --name docker-box --allow friend@example.com \
  --driver-bin "$BIN/stub-driver" --runner "$RUNNER"
pass "setup ($RUNNER)"

STATUS="$(cua --json host status)"
echo "$STATUS"
echo "$STATUS" | grep -q '"configured":true' || fail "not configured"
echo "$STATUS" | grep -q '"running":true' || fail "service not running"
echo "$STATUS" | grep -q "\"kind\":\"$RUNNER\"" || fail "wrong runner"
MACHINE="$(echo "$STATUS" | sed -n 's/.*"machineId":"\([a-z0-9-]*\)".*/\1/p')"
[ -n "$MACHINE" ] || fail "no machine id"
pass "status: configured, $RUNNER service running, machine $MACHINE"

for _ in $(seq 1 50); do [ -s /tmp/stub-driver.args ] && break; sleep 0.1; done
ARGS="$(cat /tmp/stub-driver.args)"
echo "driver args: $ARGS"
case "$ARGS" in
  "join --relay $RELAY --relay-token-file /root/.cua/host/machine-token --host-policy /root/.cua/host/host.json --machine-id-file /root/.cua/spacesd/id --relay-jwks /root/.cua/host/relay-jwks.json --token-file /root/.cua/host/env-token --data-dir /root/.cua/spacesd") ;;
  *) fail "unexpected driver command line" ;;
esac
[ "$(stat -c %a /root/.cua/host/machine-token)" = 600 ] || fail "machine token not 0600"
grep -q '"owner": "user-1"' /root/.cua/host/host.json || fail "policy owner"
[ "$(cat /root/.cua/spacesd/id)" = "$MACHINE" ] || fail "id file"
pass "driver runs join with the machine token, policy and id"

if [ "$RUNNER" = systemd ]; then
  systemctl is-enabled cua-spacesd-host >/dev/null || fail "unit not enabled"
  grep -q '^WantedBy=multi-user.target' /etc/systemd/system/cua-spacesd-host.service || fail "system unit"
  pass "systemd system unit enabled"
fi

cua spaces ls | tee /tmp/spaces.txt
grep -q "relay:$MACHINE" /tmp/spaces.txt || fail "spaces ls lacks the relay machine"
pass "spaces ls shows space://relay/$MACHINE"

cua --json host stop | grep -q '"sharing":false' || fail "stop"
grep -q '"sharing": false' /root/.cua/host/host.json || fail "policy sharing"
pass "stop sharing"
cua --json host start | grep -q '"sharing":true' || fail "start"
pass "start sharing"

DRIVER_PID="$(pgrep -f "$BIN/stub-driver" | head -n1 || true)"
[ -n "$DRIVER_PID" ] || DRIVER_PID="$(pgrep -f 'sleep 100000' | head -n1 || true)"
cua host remove --force
sleep 1
if pgrep -f 'sleep 100000' >/dev/null; then fail "driver still running after remove"; fi
[ ! -e /root/.cua/host ] || fail "host dir left behind"
if [ "$RUNNER" = systemd ] && [ -e /etc/systemd/system/cua-spacesd-host.service ]; then fail "unit left"; fi
if cua host status; then fail "status should exit 1 after remove"; fi
pass "remove: driver stopped, service and host state gone"

kill "$RELAY_PID" 2>/dev/null || true
echo "ALL PASSED ($RUNNER)"
