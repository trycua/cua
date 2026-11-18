#!/bin/sh
# session.sh DURATION_S BASE_URL NPAGES — one "browser session" inside a Space:
# a headless Chromium kept open for DURATION_S, opening a new page every ~10 s (CDP over HTTP),
# like a short scraping session. Prints one JSON line with what it did.
D=${1:-180}; BASE=${2:?base URL of the test site}; K=${3:-10}; STEP=${STEP:-10}
P=9333
T0=$(date +%s)
PROF=$(mktemp -d)
chromium --headless=new --no-sandbox --disable-gpu --disable-dev-shm-usage --no-first-run \
  --remote-debugging-port=$P --user-data-dir=$PROF about:blank >/dev/null 2>&1 &
CP=$!
i=0
while [ $i -lt 100 ]; do curl -fs http://127.0.0.1:$P/json/version >/dev/null 2>&1 && break; sleep 0.1; i=$((i+1)); done
READY=$(( $(date +%s) - T0 ))
n=0; ok=0; rendered=0; prev=""
END=$((T0 + D))
while [ "$(date +%s)" -lt "$END" ]; do
  url="$BASE/p$((n % K)).html"
  r=$(curl -fs -X PUT "http://127.0.0.1:$P/json/new?$url")
  id=$(printf '%s' "$r" | sed -n 's/.*"id": *"\([^"]*\)".*/\1/p' | head -1)
  [ -n "$id" ] && ok=$((ok+1))
  [ -n "$prev" ] && curl -fs "http://127.0.0.1:$P/json/close/$prev" >/dev/null 2>&1
  prev=$id; n=$((n+1))
  left=$((END - $(date +%s))); [ $left -gt $STEP ] && left=$STEP; [ $left -gt 0 ] && sleep $left
  # rendered = our page script ran (it appends " <N>ms" to the title)
  curl -fs http://127.0.0.1:$P/json/list | grep -q '[0-9]ms",' && rendered=$((rendered+1))
done
TITLES=$(curl -fs http://127.0.0.1:$P/json/list | grep -c '"title"')
kill $CP 2>/dev/null; wait $CP 2>/dev/null; rm -rf $PROF
echo "{\"session_s\":$(( $(date +%s) - T0 )),\"browser_ready_s\":$READY,\"pages\":$n,\"pages_ok\":$ok,\"rendered\":$rendered,\"open_tabs\":$TITLES}"
