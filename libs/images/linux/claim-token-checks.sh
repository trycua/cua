#!/usr/bin/env bash
# Shared checks for the Fleet claim token smoke tests (sourced by
# smoke-claim-token.sh and smoke-claim-token-vm.sh). The caller sets URL and
# WORK and defines set_token TOKEN / clear_token (the operator's side), plus
# ok/bad. Everything talks gRPC-Web over curl from the host.

# grpc_web METHOD BODY_HEX [TOKEN] -> prints the grpc-status (header for
# trailers-only errors, else the trailer frame in the body).
grpc_web() {
    local method="$1" hex="$2" token="${3:-}" len s
    len=$(( ${#hex} / 2 ))
    printf '%s' "00$(printf '%08x' "$len")$hex" | xxd -r -p >"$WORK/req"
    local auth=()
    [ -n "$token" ] && auth=(-H "x-cua-env-authorization: Bearer $token")
    curl -s -m 10 -D "$WORK/hdr" -o "$WORK/body" -X POST "$URL/cua.env.v1.$method" \
        -H 'content-type: application/grpc-web+proto' -H 'x-grpc-web: 1' \
        ${auth[@]+"${auth[@]}"} --data-binary @"$WORK/req" || { echo "curl-failed"; return; }
    s="$(tr -d '\r' <"$WORK/hdr" | awk -F': ' 'tolower($1)=="grpc-status"{print $2}' | tail -1)"
    [ -z "$s" ] && s="$(LC_ALL=C grep -ao 'grpc-status:[0-9]*' "$WORK/body" | tail -1 | cut -d: -f2)"
    echo "${s:-none}"
}

# StatRequest{path:"/"} and InitRequest{token:"attacker-..."} (field 1 both).
STAT_HEX="0a012f"
INIT_TOKEN="attacker-0123456789abcdef"
INIT_HEX="0a$(printf '%02x' ${#INIT_TOKEN})$(printf '%s' "$INIT_TOKEN" | xxd -p | tr -d '\n')"

# expect DESC WANT METHOD HEX [TOKEN]: polls up to 30 s for grpc-status WANT.
expect() {
    local desc="$1" want="$2" got=""
    shift 2
    for _ in $(seq 1 60); do
        got="$(grpc_web "$@")"
        [ "$got" = "$want" ] && { ok "$desc (grpc-status $got)"; return; }
        sleep 0.5
    done
    bad "$desc: wanted grpc-status $want, got $got"
}

# wait_driver SECONDS: until GetCapabilities answers.
wait_driver() {
    local i
    for i in $(seq 1 "$1"); do
        [ "$(grpc_web SystemService/GetCapabilities "")" = 0 ] && return 0
        sleep 1
    done
    return 1
}

claim_awaiting_checks() {
    expect "Health answers while awaiting" 0 SystemService/Health ""
    expect "Stat without a token is FAILED_PRECONDITION" 9 FilesystemService/Stat "$STAT_HEX"
    expect "Stat with a guessed token is FAILED_PRECONDITION" 9 FilesystemService/Stat "$STAT_HEX" "$TOKEN_A"
    expect "Init carrying a token is refused (no network token)" 9 SystemService/Init "$INIT_HEX"
}

claim_install_checks() {
    set_token "$TOKEN_A"
    expect "Stat with token A works" 0 FilesystemService/Stat "$STAT_HEX" "$TOKEN_A"
    expect "Stat without a token is UNAUTHENTICATED" 16 FilesystemService/Stat "$STAT_HEX"
}

claim_rotate_revoke_checks() {
    echo "==> rotation: token B"
    set_token "$TOKEN_B"
    expect "Stat with token B works" 0 FilesystemService/Stat "$STAT_HEX" "$TOKEN_B"
    expect "Stat with old token A is UNAUTHENTICATED" 16 FilesystemService/Stat "$STAT_HEX" "$TOKEN_A"
    echo "==> release: file emptied"
    clear_token
    expect "Stat with token B is FAILED_PRECONDITION again" 9 FilesystemService/Stat "$STAT_HEX" "$TOKEN_B"
    expect "GetCapabilities still answers" 0 SystemService/GetCapabilities ""
    echo "==> next claim: token A again"
    set_token "$TOKEN_A"
    expect "Stat with token A works again" 0 FilesystemService/Stat "$STAT_HEX" "$TOKEN_A"
}
