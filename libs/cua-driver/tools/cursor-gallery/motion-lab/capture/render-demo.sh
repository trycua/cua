#!/usr/bin/env bash
# Render the motion-lab demo media: gallery videos, per-candidate clips,
# a timing comparison and contact sheets. H.264 MP4s sized for social posts.
#
#   motion-lab/capture/render-demo.sh OUT_DIR [candidate-id ...]
#
# Needs Chrome, Node 22+ (global WebSocket), Python 3 and ffmpeg.
set -euo pipefail

LAB_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
GALLERY_DIR=$(dirname "$LAB_DIR")
OUT=${1:?usage: render-demo.sh OUT_DIR [ids...]}
shift || true
CLIPS=("$@")
if [[ ${#CLIPS[@]} -eq 0 ]]; then
  CLIPS=(dc-signature-arc dc-spring-settle dc-comet-swoop dc-anticipate dc-magnetic dc-bank-glide
         dc-lift-hop dc-studio-smooth dc-weave dc-calm-breath dc-squash-pop dc-think-loop)
fi
HTTP_PORT=${MOTION_LAB_PORT:-3091}
CDP_PORT=${MOTION_LAB_CDP_PORT:-9391}
for port in "$HTTP_PORT" "$CDP_PORT"; do
  if lsof -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1; then echo "render-demo: port $port is busy" >&2; exit 1; fi
done
CHROME=${CURSOR_GALLERY_CHROME:-"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"}
WORK=$(mktemp -d)
mkdir -p "$OUT/clips"

python3 -m http.server "$HTTP_PORT" --bind 127.0.0.1 --directory "$GALLERY_DIR" >"$WORK/http.log" 2>&1 &
HTTP_PID=$!
"$CHROME" --headless=new --disable-gpu --hide-scrollbars --remote-debugging-port="$CDP_PORT" \
  --user-data-dir="$WORK/chrome" about:blank >"$WORK/chrome.log" 2>&1 &
CHROME_PID=$!
trap 'kill "$HTTP_PID" "$CHROME_PID" 2>/dev/null || true; rm -rf "$WORK"' EXIT
for _ in $(seq 1 30); do curl -fsS "http://127.0.0.1:$CDP_PORT/json/version" >/dev/null 2>&1 && break; sleep 0.5; done
for _ in $(seq 1 30); do curl -fsS "http://127.0.0.1:$HTTP_PORT/motion-lab/" >/dev/null 2>&1 && break; sleep 0.5; done

export CDP_ENDPOINT="http://127.0.0.1:$CDP_PORT"
BASE="http://127.0.0.1:$HTTP_PORT/motion-lab/?capture=1"
REC="node $LAB_DIR/capture/record.mjs"

encode() { # frames_dir out.mp4
  ffmpeg -y -loglevel error -framerate 30 -i "$1/%05d.jpg" -c:v libx264 -preset slow -crf "${CRF:-24}" \
    -pix_fmt yuv420p -movflags +faststart "$2"
}

video() { # name url seconds
  $REC video "$2" "$WORK/$1" --w 1920 --h 1080 --fps 30 --seconds "$3"
  encode "$WORK/$1" "$OUT/$1.mp4"
  rm -rf "${WORK:?}/$1"
}

video gallery-directors-cut "$BASE&set=dc&timing=fitts&cols=4&layout=big&title=12%20agent%20cursor%20%3Cem%3Emotion%20styles%3C/em%3E" 15 &
video gallery-all "$BASE&set=showcase&timing=fitts&cols=10&layout=compact" 15 &
video timing-compare "$BASE&layout=big&cols=3&ids=dubins-glide@native,heading-candidates@native,dc-signature-arc@fitts&title=Distance-aware%20%3Cem%3Etiming%3C/em%3E&sub=Left%3A%20Cua%20Driver%20today.%20Middle%3A%20Codex-like%20fixed%201.4%20s%20spring.%20Right%3A%20Signature%20arc%20with%20Fitts%20timing." 14 &
wait

i=0
for id in "${CLIPS[@]}"; do
  i=$((i + 1))
  video "clips/$(printf '%02d' "$i")-$id" "$BASE&layout=clipmode#/clip/$id" 12 &
  if (( i % 4 == 0 )); then wait; fi
done
wait

$REC shot "$BASE&set=showcase&timing=fitts&cols=8#/sheet" "$OUT/contact-sheet-all.png" --w 2400 --h 3000 --t 0
$REC shot "$BASE&set=dc&timing=fitts&cols=4&layout=big#/sheet" "$OUT/contact-sheet-directors-cut.png" --w 1920 --h 1080 --t 0
echo "render-demo: wrote $OUT"
