#!/bin/bash
# run.sh KIND RUNTIME COUNT CONCURRENCY CPUS MEMORY_MB SESSION_S IMAGE [cua sb run args...]
#
# Submits COUNT browser sessions to all your machines (`cua sb run --on auto`), each in a fresh Space that is deleted
# when the session ends. Each Space fetches session.sh from $SITE/s.sh (put session.sh there next to
# the pages make-site.py writes), so every job's command stays small. Prints the run ids.
#
#   SITE=http://10.0.0.5:8088 ./run.sh container gvisor 40000 3000 0.5 1024 180 \
#     ghcr.io/trycua/linux:24.04-slim --os linux --arch x86_64
set -euo pipefail
export DO_NOT_TRACK=${DO_NOT_TRACK:-1}
KIND=$1; RT=$2; COUNT=$3; CONC=$4; CPUS=$5; MEM=$6; SES=$7; IMG=$8
shift 8
: "${SITE:?set SITE to the base URL of the test site, reachable from inside the Spaces}"
BATCH=${BATCH:-5000}
left=$COUNT
while [ "$left" -gt 0 ]; do
  n=$(( left < BATCH ? left : BATCH ))
  cua sb run --on "${ON:-auto}" --image "$IMG" --kind "$KIND" --runtime "$RT" --count "$n" --concurrency "$CONC" \
    --cpus "$CPUS" --memory "$MEM" --timeout 15m --detach "$@" \
    -- sh -c "curl -fsS $SITE/s.sh -o /tmp/s.sh && sh /tmp/s.sh $SES $SITE 10" | tail -1
  left=$(( left - n ))
done
