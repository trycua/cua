#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
#
# A local relay, a fake identity provider and one machine's daemon, in an
# isolated home, for end-to-end runs of the phone app. Nothing here touches
# your real ~/.cua, the installed app, the keychain, ~/.claude, ~/.codex,
# ~/.aws or any production service: every file lives under $HARNESS_DIR,
# every port is on loopback, and credentials are a 0600 file there.
#
#   harness.sh start    build what is missing, start the relay and the machine
#   harness.sh stop     stop what start started (only those processes)
#   harness.sh status   what is running
#   harness.sh env      the variables a client needs, as `export` lines
#   harness.sh token [sub]   a fresh account token (default: ada)
#   harness.sh clean    stop, then delete $HARNESS_DIR
#
# Variables: HARNESS_DIR (default ~/projects/.cua-work/mobile-gaps/harness),
# HARNESS_MACHINE (the machine's name, default harness-machine),
# HARNESS_CARGO_JOBS (default 6), CARGO_TARGET_DIR_SPACESD and
# CARGO_TARGET_DIR_CUA (separate target directories; default each
# workspace's own).
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
libs="$(cd "$here/../../../.." && pwd)"
D="${HARNESS_DIR:-$HOME/projects/.cua-work/mobile-gaps/harness}"
MACHINE="${HARNESS_MACHINE:-harness-machine}"
JOBS="${HARNESS_CARGO_JOBS:-6}"
sp_target="${CARGO_TARGET_DIR_SPACESD:-$libs/cua-spacesd/target}"
cua_target="${CARGO_TARGET_DIR_CUA:-$libs/cua/target}"
profile=debug

relay_dir="$D/relay"; cua_home="$D/cua-home"; fake_home="$D/home"; bin="$D/bin"; logs="$D/logs"

die() { echo "harness: $*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "$1 is required"; }

json_get() { python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))[sys.argv[2]])' "$1" "$2"; }

# The environment every child of the machine side runs in: an isolated
# home, the local relay, the local identity provider, a credential file.
machine_env() {
  local relay issuer
  relay="$(json_get "$relay_dir/harness.json" relay_url)"
  issuer="$(json_get "$relay_dir/harness.json" issuer)"
  cat <<ENV
HOME=$fake_home
CUA_HOME=$cua_home
CUA_CREDENTIAL_STORE=file
CUA_RELAY_URL=$relay
CUA_OIDC_ISSUER=$issuer
CUA_MESH=off
CUA_TELEMETRY=off
CUA_NO_UPDATE_CHECK=1
CUA_KEYCHAIN_NONINTERACTIVE=1
CUA_BIN=$bin/cua
PATH=$bin:/usr/bin:/bin:/usr/sbin:/sbin:/usr/local/bin:/opt/homebrew/bin
ENV
}

run_machine() { # command...
  # shellcheck disable=SC2046
  env -i $(machine_env | tr '\n' ' ') "$@"
}

alive() { [ -f "$1" ] && kill -0 "$(cat "$1")" 2>/dev/null; }

build() {
  need cargo; need python3
  mkdir -p "$bin"
  echo "== building (relay example, cua-spacesd, cua)"
  (cd "$libs/cua-spacesd" && CARGO_TARGET_DIR="$sp_target" CARGO_INCREMENTAL=0 \
     cargo build -j "$JOBS" -p cua-relay --example local-harness &&
     CARGO_TARGET_DIR="$sp_target" CARGO_INCREMENTAL=0 cargo build -j "$JOBS" -p cua-spacesd)
  (cd "$libs/cua" && CARGO_TARGET_DIR="$cua_target" CARGO_INCREMENTAL=0 \
     cargo build -j "$JOBS" -p cua-spaces-cli)
  cp "$sp_target/$profile/examples/local-harness" "$bin/local-harness"
  cp "$sp_target/$profile/cua-spacesd" "$bin/cua-spacesd"
  cp "$cua_target/$profile/cua-spaces-cli" "$bin/cua"
}

seed_credentials() { # a signed-in account for the machine side
  local sub=ada exp tok
  tok="$(mint "$sub")"
  exp="$(python3 - <<'PY'
import datetime
print((datetime.datetime.now(datetime.timezone.utc)+datetime.timedelta(hours=11)).strftime("%Y-%m-%dT%H:%M:%SZ"))
PY
)"
  mkdir -p "$cua_home"
  (umask 077; python3 - "$cua_home/credentials.json" "$tok" "$exp" "$sub" <<'PY'
import json, sys
path, tok, exp, sub = sys.argv[1:5]
json.dump({"access_token": tok, "refresh_token": "refresh:" + sub, "expires_at": exp,
           "token_type": "Bearer", "scope": "openid profile offline_access"}, open(path, "w"))
PY
  )
  chmod 600 "$cua_home/credentials.json"
}

mint() { # sub -> access token
  local sub="${1:-ada}" url
  url="$(json_get "$relay_dir/harness.json" mint_url)"
  curl -fsS -X POST "$url" -H 'content-type: application/json' -d "{\"sub\":\"$sub\"}" |
    python3 -c 'import json,sys; print(json.load(sys.stdin)["access_token"])'
}

start() {
  [ "$(id -u)" != 0 ] || die "do not run this as root"
  mkdir -p "$D" "$relay_dir" "$cua_home" "$fake_home" "$bin" "$logs"
  chmod 700 "$D"
  if alive "$D/relay.pid"; then echo "already running (stop first)"; status; return 0; fi
  [ -x "$bin/local-harness" ] && [ -x "$bin/cua" ] && [ -x "$bin/cua-spacesd" ] || build
  rm -f "$relay_dir/harness.json"
  echo "== starting the local relay"
  ( "$bin/local-harness" --dir "$relay_dir" --accounts ada,bob >"$logs/relay.out" 2>"$logs/relay.err" &
    echo $! >"$D/relay.pid" )
  for _ in $(seq 1 100); do [ -s "$relay_dir/harness.json" ] && break; sleep 0.2; done
  [ -s "$relay_dir/harness.json" ] || { cat "$logs/relay.err" >&2; die "the relay did not start"; }
  seed_credentials
  echo "== setting up the machine \"$MACHINE\" (process runner, Spaces only; no service is installed)"
  run_machine "$bin/cua" host setup --runner process --share spaces --name "$MACHINE" \
    --driver-bin "$bin/cua-spacesd" >"$logs/host-setup.out" 2>&1 || { cat "$logs/host-setup.out" >&2; stop; die "host setup failed"; }
  # A machine stopped before (stop-sharing at the relay) shares again.
  run_machine "$bin/cua" host start >>"$logs/host-setup.out" 2>&1 || true
  echo "== starting the machine's daemon"
  ( run_machine "$bin/cua" daemon start --foreground >"$logs/daemon.out" 2>&1 &
    echo $! >"$D/daemon.pid" )
  for _ in $(seq 1 150); do [ -S "$cua_home/cua.sock" ] && break; sleep 0.2; done
  [ -S "$cua_home/cua.sock" ] || { tail -20 "$logs/daemon.out" >&2; stop; die "the daemon did not start"; }
  write_env
  status
}

write_env() {
  local relay issuer mint
  relay="$(json_get "$relay_dir/harness.json" relay_url)"
  issuer="$(json_get "$relay_dir/harness.json" issuer)"
  mint="$(json_get "$relay_dir/harness.json" mint_url)"
  cat >"$D/env.sh" <<ENV
# source this: the local relay for the phone app (loopback only, fake accounts)
export CUA_HARNESS_DIR='$D'
export CUA_HARNESS_RELAY_URL='$relay'
export CUA_HARNESS_ISSUER='$issuer'
export CUA_HARNESS_MINT_URL='$mint'
export CUA_HARNESS_PUSH_ALERTS='$relay_dir/push-alerts.jsonl'
export CUA_HARNESS_MACHINE='$MACHINE'
ENV
}

status() {
  for p in relay daemon; do
    if alive "$D/$p.pid"; then echo "$p: running (pid $(cat "$D/$p.pid"))"; else echo "$p: stopped"; fi
  done
  if [ -s "$relay_dir/harness.json" ]; then
    echo "relay: $(json_get "$relay_dir/harness.json" relay_url)"
    echo "identity provider: $(json_get "$relay_dir/harness.json" idp_url) (POST /token {\"sub\":\"ada\"})"
    if [ -S "$cua_home/cua.sock" ]; then
      run_machine "$bin/cua" host status 2>&1 | head -12 || true
    fi
  fi
  echo "env: $D/env.sh"
}

stop() {
  # Only what start started: the daemon, the machine's service (a process
  # runner's own pidfile) and the relay.
  if [ -x "$bin/cua" ] && [ -s "$relay_dir/harness.json" ]; then
    run_machine "$bin/cua" host stop >/dev/null 2>&1 || true
    run_machine "$bin/cua" daemon stop >/dev/null 2>&1 || true
  fi
  for p in daemon relay; do
    if alive "$D/$p.pid"; then kill "$(cat "$D/$p.pid")" 2>/dev/null || true; fi
  done
  for _ in $(seq 1 50); do
    alive "$D/daemon.pid" || alive "$D/relay.pid" || break
    sleep 0.2
  done
  for p in daemon relay; do
    if alive "$D/$p.pid"; then kill -9 "$(cat "$D/$p.pid")" 2>/dev/null || true; fi
    rm -f "$D/$p.pid"
  done
  # The machine's driver (cua-spacesd join) has its own pidfile under the
  # isolated cua home: stop exactly that one.
  for f in "$cua_home"/host/*.pid; do
    [ -f "$f" ] || continue
    pid="$(cat "$f")"
    case "$(ps -p "$pid" -o command= 2>/dev/null || true)" in
      *"$bin/cua-spacesd"*) kill "$pid" 2>/dev/null || true ;;
    esac
  done
  echo "stopped"
}

case "${1:-}" in
  start) start ;;
  stop) stop ;;
  status) status ;;
  env) [ -f "$D/env.sh" ] || die "not started"; cat "$D/env.sh" ;;
  token) mint "${2:-ada}" ;;
  clean) stop; rm -rf "$D" ;;
  *) sed -n '5,22p' "$0"; exit 2 ;;
esac
