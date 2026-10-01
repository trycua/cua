#!/usr/bin/env bash
# Run `cua-spacesd doctor --strict` in a running macOS Lume VM (over
# `lume ssh`, user lume) and save the report; then, with --token-file, the
# accessibility/input probe (tools/ax_probe.py) from the host against the
# VM's /mcp. Exit 0 only when both pass. build.sh runs it after the
# post-install reboot; scripts/images/image-doctor-lume.sh runs it against a
# pulled image.
#
#   doctor-gate.sh --vm NAME --out DIR [--token-file HOST_FILE]
#       [--effects virtual|none] [--no-strict] [--expect COMPONENT=WANT]...
#
# The service must have a token (the Lume setup share, as the cua SDK
# delivers it); the doctor reads it from ~/.cua/spacesd/token. The probe
# needs uv on the host.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VM="" OUT="" TOKEN_FILE="" EFFECTS=virtual STRICT=--strict EXPECT=()
while [ $# -gt 0 ]; do
    case "$1" in
        --vm) VM="$2"; shift 2 ;;
        --out) OUT="$2"; shift 2 ;;
        --token-file) TOKEN_FILE="$2"; shift 2 ;;
        --effects) EFFECTS="$2"; shift 2 ;;
        --no-strict) STRICT=""; shift ;;
        --expect) EXPECT+=(--expect "$2"); shift 2 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
[ -n "$VM" ] && [ -n "$OUT" ] || { echo "--vm and --out are required" >&2; exit 2; }
mkdir -p "$OUT"
guest() { lume ssh "$VM" -t "${2:-120}" "$1" </dev/null; }

# The LaunchAgent starts at login; wait (bounded) for :3211 to answer.
ready=0
for _ in $(seq 1 60); do
    if guest 'curl -s -o /dev/null -m 3 -w "%{http_code}" http://127.0.0.1:3211/health' 2>/dev/null | grep -q '^204'; then
        ready=1; break
    fi
    sleep 5
done
[ "$ready" = 1 ] || echo "warning: cua-spacesd /health never answered 204; running the doctor anyway" >&2

set +e
guest "rm -rf /tmp/cua-doctor && mkdir -p /tmp/cua-doctor && /usr/local/bin/cua-spacesd doctor --format human \
    --effects $EFFECTS $STRICT --expect-runtime lume ${EXPECT[*]:-} --out /tmp/cua-doctor/report.json \
    --junit /tmp/cua-doctor/report.xml --artifacts /tmp/cua-doctor/artifacts 2>/tmp/cua-doctor/stderr.txt \
    >/tmp/cua-doctor/report.txt; echo \$? >/tmp/cua-doctor/exit" 600 >/dev/null
set -e
rc="$(guest 'cat /tmp/cua-doctor/exit' | tr -dc '0-9')"
# Copy the report out as base64 text (lume ssh carries text).
for f in report.json report.xml report.txt stderr.txt artifacts/screenshot.display.png; do
    guest "test -f /tmp/cua-doctor/$f && base64 -i /tmp/cua-doctor/$f" 2>/dev/null |
        base64 -d >"$OUT/$(basename "$f")" 2>/dev/null || rm -f "$OUT/$(basename "$f")"
done
guest '/usr/local/bin/cua-spacesd build-info' >"$OUT/build-info.json" 2>/dev/null || true
tail -1 "$OUT/report.txt" 2>/dev/null || true
rc="${rc:-2}"

probe=skipped
if [ -n "$TOKEN_FILE" ]; then
    ip="$(lume get "$VM" --format json | python3 -c 'import json,sys;d=json.load(sys.stdin);d=d[0] if isinstance(d,list) else d;print(d.get("ipAddress") or "")')"
    # One retry after a pause: right after a login the guest can still be
    # busy enough that a posted click times out (the retry's record wins;
    # the first is kept as ax-probe.first.json).
    probe=fail
    for attempt in 1 2; do
        if timeout 300 uv run -q --no-project --with 'mcp>=1.12,<1.14' python "$HERE/tools/ax_probe.py" \
            "http://$ip:3211/mcp" "$TOKEN_FILE" "$OUT/ax-probe.json"; then
            probe=pass; break
        fi
        [ "$attempt" = 2 ] || { mv "$OUT/ax-probe.json" "$OUT/ax-probe.first.json" 2>/dev/null; sleep 60; }
    done
    [ "$probe" = pass ] || [ "$rc" != 0 ] || rc=1
fi
printf '{"lane": "lume", "arch": "arm64", "variant": "lume", "vm": "%s", "exit": %s, "ax_probe": "%s"}\n' \
    "$VM" "$rc" "$probe" >"$OUT/lane.json"
exit "$rc"
