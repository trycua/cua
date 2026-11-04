#!/usr/bin/env bash
# macOS lane of the image doctor (Lume only, self-hosted Apple Silicon): pull
# the image into a cua-e2e VM, boot it headless with a token in the Lume setup
# share (as the cua SDK starts it), and, when the image ships cua-spacesd, run
# libs/images/macos/doctor-gate.sh (`cua-spacesd doctor` in the guest over
# `lume ssh`, then the accessibility/input probe); otherwise the doctor shim
# through the image's computer-server (needs --cua). Claims:
# scripts/images/manifests/macos-26.json. The VM is always stopped and
# deleted. macOS allows two VMs per host; run one lane at a time.
#
#   image-doctor-lume.sh --image REF --out DIR [--strict] [--cua CUA_BIN]
#       [--manifest FILE] [--memory 4GB] [--timeout 900]
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IMAGE="" CUA="" OUT="$PWD/doctor-out/lume" STRICT=() MANIFEST="$HERE/manifests/macos-26.json"
MEMORY=4GB TIMEOUT=900
while [ $# -gt 0 ]; do
    case "$1" in
        --image) IMAGE="$2"; shift 2 ;;
        --cua) CUA="$2"; shift 2 ;;
        --out) OUT="$2"; shift 2 ;;
        --strict) STRICT=(--strict); shift ;;
        --manifest) MANIFEST="$2"; shift 2 ;;
        --memory) MEMORY="$2"; shift 2 ;;
        --timeout) TIMEOUT="$2"; shift 2 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
[ -n "$IMAGE" ] || { echo "--image is required" >&2; exit 2; }
mkdir -p "$OUT"
NAME="cua-e2e-doctor-lume-$$"
SETUP="$(mktemp -d "${TMPDIR:-/tmp}/cua-doctor-lume.XXXXXX")/setup"
mkdir -m 700 "$SETUP"
(umask 077; openssl rand -hex 32 >"$SETUP/env-token")
cleanup() {
    rm -rf "$(dirname "$SETUP")"
    lume stop "$NAME" >/dev/null 2>&1 || true
    lume delete "$NAME" --force >/dev/null 2>&1 || true
}
trap cleanup EXIT

# `lume pull` takes name:tag relative to --registry/--organization.
ref="${IMAGE#ghcr.io/}"; org="${ref%%/*}"; image="${ref#*/}"
lume pull "$image" "$NAME" --registry ghcr.io --organization "$org"
lume set "$NAME" --memory "$MEMORY"
lume run "$NAME" --display none --detach --shared-dir "$SETUP:ro" --log-file "$OUT/lume.log"
ip=""
for _ in $(seq 1 $((TIMEOUT / 5))); do
    ip="$(lume get "$NAME" --format json | python3 -c 'import json,sys;d=json.load(sys.stdin);d=d[0] if isinstance(d,list) else d;print(d.get("ipAddress") or "")')"
    [ -n "$ip" ] && break
    sleep 5
done
[ -n "$ip" ] || { echo "the VM never got an IP" >&2; exit 2; }
guest() { lume ssh "$NAME" -t "${2:-60}" "$1" </dev/null; }
for _ in $(seq 1 $((TIMEOUT / 5))); do guest 'echo up' 2>/dev/null | grep -q up && break; sleep 5; done
set +e
if guest 'test -x /usr/local/bin/cua-spacesd' >/dev/null 2>&1; then
    gate=()
    [ ${#STRICT[@]} -gt 0 ] || gate=(--no-strict)
    # Writes report.{json,xml,txt}, ax-probe.json and lane.json into $OUT.
    "$HERE/../../libs/images/macos/doctor-gate.sh" --vm "$NAME" --out "$OUT" \
        --token-file "$SETUP/env-token" ${gate[@]+"${gate[@]}"}
    exit $?
else
    [ -x "$CUA" ] || { echo "the image has no cua-spacesd: pass --cua for the computer-server shim" >&2; exit 2; }
    url="http://$ip:8000"
    for _ in $(seq 1 $((TIMEOUT / 5))); do curl -fs -m 5 "$url/status" >/dev/null 2>&1 && break; sleep 5; done
    "$CUA" doctor --no-host --shim "computer-server=$url" --expect-manifest "$MANIFEST" --effects virtual \
        ${STRICT[@]+"${STRICT[@]}"} --out "$OUT/report.json" --junit "$OUT/report.xml" | tee "$OUT/report.txt"
    rc=${PIPESTATUS[0]}
fi
set -e
printf '{"lane": "lume", "arch": "arm64", "variant": "lume", "claim_secrets": false, "exit": %s}\n' "$rc" >"$OUT/lane.json"
exit "$rc"
