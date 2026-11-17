#!/usr/bin/env bash
#
# A throwaway Linux Space for verification: Xvfb + openbox + (optional)
# PipeWire audio + the x11-pad input-echo fixture + a source-built
# cua-spacesd, registered as a Space in an isolated cua home. Drive it with
# the source-built `cua` (cua-spaces-cli) and the HTML5 viewer.
#
#   spaces-desk.sh build                 build cua-spacesd, x11-pad and cua (release)
#   spaces-desk.sh up [--no-audio]       start a Space; prints RUN=<state dir> and READY
#   spaces-desk.sh doctor RUN            read-only: is this instance worth driving?
#   spaces-desk.sh cua RUN -- ARGS...    run `cua ARGS` against RUN, transcript into the evidence dir
#   spaces-desk.sh viewer RUN NAME [QUERY]  open the Space in headless Chromium, click and type, capture
#   spaces-desk.sh index RUN             `cua viewer`'s index page: does it list the Space?
#   spaces-desk.sh down RUN              stop what `up` started; keeps the evidence
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SPACESD="$ROOT/libs/cua-spacesd/target/release/cua-spacesd"
PAD="$ROOT/libs/cua-spacesd/target/release/cua-spacesd-x11-pad"
CUA="$ROOT/libs/cua/target/release/cua-spaces-cli"
die() { echo "spaces-desk: $*" >&2; exit 1; }

load() {
    local run="${1:-}"
    [ -n "$run" ] && [ -f "$run/state.env" ] || die "no state at '${run}' (pass the RUN that 'up' printed)"
    # shellcheck disable=SC1091
    . "$run/state.env"
}

# The isolated environment every client command runs in.
client_env() {
    export HOME="$RUN/home" CUA_HOME="$RUN/home/.cua" DISPLAY="$DISPLAY_ID" \
        XDG_RUNTIME_DIR="$RUN/xdg" DO_NOT_TRACK=1 CUA_TELEMETRY=0
    unset CUA_DAEMON CUA_ENV_TOKEN
}

cmd_build() {
    (cd "$ROOT/libs/cua-spacesd" && cargo build --locked --release -p cua-spacesd --bin cua-spacesd \
        && cargo build --locked --release -p cua-spacesd-test-apps --bin cua-spacesd-x11-pad)
    (cd "$ROOT/libs/cua" && cargo build --locked --release -p cua-spaces-cli)
}

start() { # name, command...: start in its own session, record the pid
    local name="$1"; shift
    setsid "$@" >"$EVIDENCE/$name.log" 2>&1 &
    echo "$!" >>"$RUN/pids"
    echo "$name $!" >>"$EVIDENCE/pids.txt"
}

cmd_up() {
    local audio=1
    [ "${1:-}" = --no-audio ] && audio=0
    for b in "$SPACESD" "$PAD" "$CUA"; do [ -x "$b" ] || die "missing $b: run '$0 build' first"; done
    command -v Xvfb >/dev/null && command -v openbox >/dev/null || die "needs Xvfb and openbox"
    RUN="$(mktemp -d "${TMPDIR:-/tmp}/cua-spaces-verify.XXXXXX")"
    mkdir -p "$ROOT/artifacts/cua-spaces/verify"
    EVIDENCE="$(mktemp -d "$ROOT/artifacts/cua-spaces/verify/run-XXXXXXXX")"
    mkdir -p "$RUN/home" "$RUN/xdg" "$RUN/data" && chmod 700 "$RUN/xdg"
    : >"$RUN/pids"
    # Display: Xvfb picks a free one and reports it.
    setsid Xvfb -displayfd 3 -screen 0 1280x800x24 -nolisten tcp 3>"$RUN/display" >"$EVIDENCE/xvfb.log" 2>&1 &
    echo "$!" >>"$RUN/pids"; echo "xvfb $!" >>"$EVIDENCE/pids.txt"
    for _ in $(seq 50); do [ -s "$RUN/display" ] && break; sleep 0.1; done
    [ -s "$RUN/display" ] || die "Xvfb did not start (see $EVIDENCE/xvfb.log)"
    DISPLAY_ID=":$(tr -d '\n' <"$RUN/display")"
    PORT="$(python3 -I -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')"
    (umask 077; od -An -tx1 -N24 /dev/urandom | tr -d ' \n' >"$RUN/token")
    export DISPLAY="$DISPLAY_ID" XDG_RUNTIME_DIR="$RUN/xdg" HOME="$RUN/home"
    # A private session bus: AT-SPI and PipeWire's session manager need one.
    DBUS_SESSION_BUS_ADDRESS="$(dbus-daemon --session --fork --print-address=1 --print-pid=3 3>"$RUN/dbus.pid")"
    export DBUS_SESSION_BUS_ADDRESS
    cat "$RUN/dbus.pid" >>"$RUN/pids"; echo "dbus $(cat "$RUN/dbus.pid")" >>"$EVIDENCE/pids.txt"
    if [ "$audio" = 1 ] && command -v pipewire >/dev/null && command -v wireplumber >/dev/null \
        && command -v pipewire-pulse >/dev/null; then
        start pipewire pipewire; sleep 0.5
        start wireplumber wireplumber
        start pipewire-pulse pipewire-pulse
        AUDIO=pipewire
    else
        AUDIO=none
    fi
    start openbox openbox
    sleep 1
    start x11-pad "$PAD" --title verify-pad --log "$EVIDENCE/pad.jsonl" --x 0 --y 0 --width 1280 --height 800 --grid
    CUA_ENV_TOKEN="$(cat "$RUN/token")" start cua-spacesd "$SPACESD" --listen "127.0.0.1:$PORT" \
        --quic-port 0 --data-dir "$RUN/data" --token-file "$RUN/no-token-file"
    URL="http://127.0.0.1:$PORT"
    cat >"$RUN/state.env" <<EOF
RUN='$RUN'
EVIDENCE='$EVIDENCE'
DISPLAY_ID='$DISPLAY_ID'
PORT='$PORT'
URL='$URL'
SPACE_ID='direct:127.0.0.1:$PORT'
SPACE_NAME='verify'
AUDIO='$AUDIO'
EOF
    cp "$RUN/state.env" "$EVIDENCE/state.env"
    # Ready means usable: /health answers and Health says the desktop serves.
    local ok=0
    for _ in $(seq 60); do
        if [ "$(curl -s -o /dev/null -w '%{http_code}' "$URL/health")" = 204 ] \
            && CUA_ENV_TOKEN="$(cat "$RUN/token")" HOME="$RUN/home" DO_NOT_TRACK=1 \
                "$CUA" spacesd call "$URL" SystemService/Health 2>/dev/null | grep -q '"HEALTH_STATUS_SERVING","components"'; then
            ok=1; break
        fi
        sleep 0.5
    done
    [ "$ok" = 1 ] || die "cua-spacesd did not become healthy (see $EVIDENCE/cua-spacesd.log); run: $0 down $RUN"
    client_env
    "$CUA" spaces add "127.0.0.1:$PORT" --token "$(cat "$RUN/token")" --name verify >"$EVIDENCE/spaces-add.log" 2>&1 \
        || die "cua spaces add failed (see $EVIDENCE/spaces-add.log); run: $0 down $RUN"
    grep -q '"ready"' "$EVIDENCE/pad.jsonl" || die "x11-pad is not up (see $EVIDENCE/x11-pad.log)"
    echo "RUN=$RUN"
    echo "EVIDENCE=$EVIDENCE"
    echo "READY space=direct:127.0.0.1:$PORT display=$DISPLAY_ID audio=$AUDIO"
}

cmd_doctor() {
    load "$1"
    local bad=0 pid
    while read -r name pid; do
        if kill -0 "$pid" 2>/dev/null; then echo "ok   $name pid $pid"; else echo "FAIL $name pid $pid is gone"; bad=1; fi
    done <"$EVIDENCE/pids.txt"
    local owner
    owner="$(ss -Hltnp "sport = :$PORT" 2>/dev/null | grep -o 'pid=[0-9]*' | head -1 | cut -d= -f2)"
    if [ "$owner" = "$(awk '$1=="cua-spacesd"{print $2}' "$EVIDENCE/pids.txt")" ]; then
        echo "ok   port $PORT is owned by our cua-spacesd"
    else
        echo "FAIL port $PORT is owned by pid '${owner}', not ours"; bad=1
    fi
    "$SPACESD" --version
    client_env
    CUA_ENV_TOKEN="$(cat "$RUN/token")" "$CUA" spacesd caps "$URL" | head -1
    "$CUA" spaces ls --json --no-size | python3 -I -c \
        'import json,sys; ids=[s["id"] for s in json.load(sys.stdin)["spaces"]]; print("spaces:", ids); sys.exit(0 if sys.argv[1] in ids else 1)' \
        "$SPACE_ID" || { echo "FAIL $SPACE_ID is not registered"; bad=1; }
    # The service's own doctor: read-only checks, report kept as evidence.
    echo "$(cat "$RUN/token")" >"$RUN/doctor-token" && chmod 600 "$RUN/doctor-token"
    "$SPACESD" doctor --target "$URL" --token-file "$RUN/doctor-token" --out "$EVIDENCE/doctor.json" \
        --artifacts "$EVIDENCE/doctor" >"$EVIDENCE/doctor.txt" 2>&1 || true
    tail -1 "$EVIDENCE/doctor.txt"
    grep -E '^\s+\[(fail|warn)\]' "$EVIDENCE/doctor.txt" || true
    return "$bad"
}

cmd_cua() {
    load "$1"; shift
    [ "${1:-}" = -- ] && shift
    client_env
    local n out err code
    n="$(find "$EVIDENCE" -maxdepth 1 -name 'cli-*.txt' | wc -l)"
    out="$EVIDENCE/cli-$(printf %03d "$n").txt"
    err="$(mktemp)"
    set +e
    "$CUA" "$@" >"$out.stdout" 2>"$err"
    code=$?
    set -e
    {
        printf '$ cua'; printf ' %q' "$@"; printf '\n'
        echo "--- stdout"; cat "$out.stdout"
        echo "--- stderr"; cat "$err"
        echo "--- exit $code"
    } >"$out"
    rm -f "$out.stdout" "$err"
    cat "$out"
    return "$code"
}

cmd_viewer() {
    load "$1"
    local name="${2:?name the capture}" query="${3:-}"
    client_env
    local pw="$RUN/pw"
    if [ ! -d "$pw/node_modules/playwright-core" ]; then
        mkdir -p "$pw" && (cd "$pw" && npm init -y >/dev/null && npm i --no-audit --no-fund --silent playwright-core@1.63.0)
    fi
    local browser="${CHROMIUM:-$(command -v chromium || command -v chromium-browser || command -v google-chrome || true)}"
    [ -n "$browser" ] || die "needs Chromium (set CHROMIUM=/path/to/chrome)"
    local link
    link="$("$CUA" sb view "$SPACE_ID" --no-open 2>&1 | grep -o 'http://[^ ]*' | head -1 || true)"
    [ -n "$link" ] || die "cua sb view printed no link"
    [ -z "$query" ] || link="$link&$query"
    PW_DIR="$pw" CHROMIUM="$browser" node "$HERE/drive-viewer.mjs" "$link" "$EVIDENCE/viewer-$name" "$EVIDENCE/pad.jsonl"
}

cmd_index() {
    load "$1"
    client_env
    local out="$EVIDENCE/viewer-index" pid url jar="$RUN/index.jar"
    setsid "$CUA" viewer --listen 127.0.0.1:0 --no-open >"$out.log" 2>&1 &
    pid=$!
    for _ in $(seq 40); do url="$(grep -o 'http://[^ ]*key=[^ ]*' "$out.log" | head -1 || true)"; [ -n "$url" ] && break; sleep 0.25; done
    if [ -n "$url" ]; then curl -s -c "$jar" -b "$jar" -L "$url" >"$out.html"; fi
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    [ -n "$url" ] || die "cua viewer printed no URL (see $out.log)"
    local want
    want="/s/$(python3 -I -c 'import sys,urllib.parse; print(urllib.parse.quote(sys.argv[1], safe=""))' "$SPACE_ID")/open"
    if grep -qF "$want" "$out.html"; then echo "PASS index lists $SPACE_ID ($out.html)"; else
        echo "FAIL index does not list $SPACE_ID: $(grep -o '<h1>.*' "$out.html" | sed 's/<[^>]*>/ /g' | head -c 200)"; return 1; fi
}

cmd_down() {
    load "$1"
    local pid
    # Kill only what `up` started, newest first.
    for pid in $(tac "$RUN/pids"); do kill "$pid" 2>/dev/null || true; done
    sleep 1
    for pid in $(tac "$RUN/pids"); do kill -9 "$pid" 2>/dev/null || true; done
    rm -rf "$RUN"
    echo "stopped; evidence kept in $EVIDENCE"
}

case "${1:-}" in
    build) cmd_build ;;
    up) shift; cmd_up "$@" ;;
    doctor) shift; cmd_doctor "$@" ;;
    cua) shift; cmd_cua "$@" ;;
    viewer) shift; cmd_viewer "$@" ;;
    index) shift; cmd_index "$@" ;;
    down) shift; cmd_down "$@" ;;
    *) sed -n '5,/^set -euo/p' "$0" | sed '$d;s/^# \{0,1\}//'; exit 2 ;;
esac
