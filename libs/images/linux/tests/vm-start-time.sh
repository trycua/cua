#!/usr/bin/env bash
# Start time of a Linux image as a local `vm` Space: the first create boots
# cold (and leaves the golden snapshot a current cua-spacesd allows); the
# next creates should restore it (a warm start). Each create waits for the
# Space's cua-spacesd, then the Space is deleted.
#
#   vm-start-time.sh --cua CUA --image IMAGE [--count 5] [--memory 2GB] [--max-warm-s S]
#
# Prints one line per create and `RESULT {json}` (cold and warm p50 in
# seconds). With --max-warm-s, exits 1 when the warm p50 is above it.
set -euo pipefail
CUA=cua; IMAGE=""; COUNT=5; MEMORY=2GB; MAX=""
while [ $# -gt 0 ]; do
    case "$1" in
        --cua) CUA="$2"; shift 2 ;;
        --image) IMAGE="$2"; shift 2 ;;
        --count) COUNT="$2"; shift 2 ;;
        --memory) MEMORY="$2"; shift 2 ;;
        --max-warm-s) MAX="$2"; shift 2 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
[ -n "$IMAGE" ] || { echo "--image is required" >&2; exit 2; }
name="start-$$"
trap '"$CUA" sb rm "$name" --force >/dev/null 2>&1 || true' EXIT
times=()
for i in $(seq 0 "$COUNT"); do
    t0=$(python3 -c 'import time; print(time.time())')
    "$CUA" sb create "$IMAGE" --kind vm --name "$name" --memory "$MEMORY" --wait tcp:3211 >/dev/null
    t=$(python3 -c "import time; print(round(time.time() - $t0, 2))")
    "$CUA" sb exec "$name" true >/dev/null
    "$CUA" sb rm "$name" --force >/dev/null
    echo "create $i: ${t}s"
    times+=("$t")
    # Let the first boot finish writing its golden snapshot.
    [ "$i" = 0 ] && sleep 5
done
python3 - "$MAX" "${times[@]}" <<'PY'
import json, statistics, sys
mx, cold, warm = sys.argv[1], float(sys.argv[2]), [float(x) for x in sys.argv[3:]]
r = {"cold_s": cold, "warm_s": warm, "warm_p50_s": statistics.median(warm) if warm else None}
print("RESULT", json.dumps(r))
if mx and warm and r["warm_p50_s"] > float(mx):
    sys.exit(f"warm p50 {r['warm_p50_s']}s is above {mx}s")
PY
