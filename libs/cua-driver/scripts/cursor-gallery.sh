#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
DRIVER_DIR=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
REPO_ROOT=$(git -C "$DRIVER_DIR" rev-parse --show-toplevel)
GALLERY_DIR="$DRIVER_DIR/tools/cursor-gallery"
TARGET_DIR="$REPO_ROOT/target/cursor-gallery"
FRAMES_DIR="$TARGET_DIR/renderer-frames"
GENERATED_DIR="$GALLERY_DIR/generated"
PORT="${CURSOR_GALLERY_PORT:-3001}"

require() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "cursor-gallery: missing required command: $1" >&2
    exit 1
  }
}

export_assets() {
  require cargo
  require ffmpeg
  mkdir -p "$FRAMES_DIR" "$GENERATED_DIR"
  cargo run --quiet \
    --manifest-path "$DRIVER_DIR/rust/Cargo.toml" \
    -p cursor-overlay \
    --example export_gallery_frames \
    --features theme-authoring \
    -- "$FRAMES_DIR"

  for group in actions previews; do
    mkdir -p "$GENERATED_DIR/$group"
    for state_dir in "$FRAMES_DIR/$group"/*; do
      state=$(basename "$state_dir")
      ffmpeg -y -loglevel error -framerate 30 \
        -i "$state_dir/%04d.png" \
        -c:v libvpx-vp9 -pix_fmt yuva420p -auto-alt-ref 0 \
        -crf 28 -b:v 0 -row-mt 1 \
        "$GENERATED_DIR/$group/$state.webm"
    done
  done
}

find_chrome() {
  if [[ -n "${CURSOR_GALLERY_CHROME:-}" ]]; then
    printf '%s\n' "$CURSOR_GALLERY_CHROME"
  elif [[ -x "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" ]]; then
    printf '%s\n' "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
  elif command -v google-chrome >/dev/null 2>&1; then
    command -v google-chrome
  elif command -v chromium >/dev/null 2>&1; then
    command -v chromium
  else
    echo "cursor-gallery: Chrome not found; set CURSOR_GALLERY_CHROME" >&2
    exit 1
  fi
}

wait_for_url() {
  local url="$1"
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    if curl -fsS "$url" >/dev/null 2>&1; then
      return
    fi
    sleep 1
  done
  echo "cursor-gallery: timed out waiting for $url" >&2
  exit 1
}

available_port() {
  python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()'
}

make_gif() {
  local name="$1"
  local frames="$2"
  local output="$3"
  local palette="$TARGET_DIR/$name-palette.png"
  ffmpeg -y -loglevel error -framerate 15 -i "$frames/%04d.png" \
    -vf "fps=15,scale=1080:-1:flags=lanczos,palettegen=stats_mode=diff" \
    "$palette"
  ffmpeg -y -loglevel error -framerate 15 -i "$frames/%04d.png" -i "$palette" \
    -lavfi "fps=15,scale=1080:-1:flags=lanczos,paletteuse=dither=bayer:bayer_scale=5:diff_mode=rectangle" \
    "$output"
}

export_docs() {
  require curl
  require node
  require python3
  export_assets

  chrome=$(find_chrome)
  http_port="${CURSOR_GALLERY_EXPORT_PORT:-$(available_port)}"
  cdp_port="${CURSOR_GALLERY_EXPORT_CDP_PORT:-$(available_port)}"
  capture_dir="$TARGET_DIR/docs-frames"
  docs_dir="$REPO_ROOT/docs/public/img/cua-driver/cursor-themes"
  profile_dir="$TARGET_DIR/chrome-profile"
  mkdir -p "$capture_dir" "$docs_dir" "$profile_dir"

  python3 -m http.server "$http_port" --bind 127.0.0.1 --directory "$REPO_ROOT" \
    >"$TARGET_DIR/server.log" 2>&1 &
  server_pid=$!
  "$chrome" --headless=new --disable-gpu --hide-scrollbars \
    --autoplay-policy=no-user-gesture-required --force-device-scale-factor=1 \
    --window-size=1600,2200 --remote-debugging-port="$cdp_port" \
    --user-data-dir="$profile_dir" about:blank \
    >"$TARGET_DIR/chrome.log" 2>&1 &
  chrome_pid=$!
  trap 'kill "$server_pid" "$chrome_pid" 2>/dev/null || true' EXIT

  wait_for_url "http://127.0.0.1:$http_port/"
  wait_for_url "http://127.0.0.1:$cdp_port/json/version"
  CDP_ENDPOINT="http://127.0.0.1:$cdp_port" \
    PAGE_URL="http://127.0.0.1:$http_port/libs/cua-driver/tools/cursor-gallery/" \
    FRAME_URL_ROOT="http://127.0.0.1:$http_port/target/cursor-gallery/renderer-frames" \
    node --experimental-websocket "$GALLERY_DIR/capture-gallery.mjs" "$capture_dir"

  make_gif \
    "action-animations" \
    "$capture_dir/actions" \
    "$docs_dir/action-animations.gif"
  echo "cursor-gallery: wrote documentation GIFs to $docs_dir"
}

promote_motion() {
  require cargo
  require python3

  local override="$TARGET_DIR/motion-override.json"
  local spec="$DRIVER_DIR/rust/crates/cursor-overlay/assets/motion.default.json"

  if [[ ! -f "$override" ]]; then
    echo "cursor-gallery: no movement override to promote ($override)." >&2
    echo "cursor-gallery: tune values in the movement lab, then re-run." >&2
    exit 1
  fi

  python3 "$GALLERY_DIR/promote_motion.py" \
    --override "$override" \
    --spec "$spec" \
    --validate-only

  python3 "$GALLERY_DIR/promote_motion.py" \
    --override "$override" \
    --spec "$spec"

  # The Rust parity tests are the promotion gate: the embedded spec must
  # still reproduce MotionConfig::default() exactly, including the
  # documented macOS/Windows+Linux spring-settle divergence. A promoted
  # value intentionally fails here until reference_defaults() in
  # src/motion.rs is updated in the same commit — double-entry review.
  echo "cursor-gallery: running the motion parity gate (expects failure until"
  echo "cursor-gallery: reference_defaults() in src/motion.rs matches the new spec)."
  if ! cargo test --quiet \
      --manifest-path "$DRIVER_DIR/rust/Cargo.toml" \
      -p cursor-overlay --lib -- motion; then
    echo "cursor-gallery: parity gate FAILED — update reference_defaults() in" >&2
    echo "cursor-gallery: rust/crates/cursor-overlay/src/motion.rs to match, then re-run." >&2
    exit 1
  fi

  echo "cursor-gallery: promoted movement override into $spec"
  echo "cursor-gallery: review the diff, then commit it as the new default."
}

case "${1:-help}" in
  assets)
    export_assets
    ;;
  serve)
    require python3
    export_assets
    echo "cursor-gallery: http://127.0.0.1:$PORT"
    exec python3 -m http.server "$PORT" --bind 127.0.0.1 --directory "$GALLERY_DIR"
    ;;
  dev)
    require cargo
    require ffmpeg
    require python3
    exec python3 "$GALLERY_DIR/dev_server.py" --port "$PORT"
    ;;
  export-docs)
    export_docs
    ;;
  promote-motion)
    promote_motion
    ;;
  *)
    echo "usage: $0 {assets|serve|dev|export-docs|promote-motion}" >&2
    exit 2
    ;;
esac
