# Native-Wayland screen-recording helper for the cua-driver Wayland TDD tests.
#
# Captures the composited Wayland output with `grim` (wlr-screencopy) in a loop
# and stitches the frames into an animated GIF. grim only works on wlroots
# compositors (labwc/wayfire/sway); on kwin/mutter it simply produces no frames
# and no GIF — which is fine, the GIF is a best-effort artifact and these tests
# are expected to fail before/around it anyway.
#
# Usage (in a test's `let`):
#   recordGifScript = import ./record-wayland-gif.nix { inherit pkgs; };
# then inside the testScript, start it in the background before driving and
# `touch` the stop-file afterwards:
#   ${recordGifScript} <frames_dir> <output_gif> <stop_file> <log_file> \
#                      <delay_cs> <interval>
# The caller must export WAYLAND_DISPLAY and XDG_RUNTIME_DIR for grim.
{ pkgs }:

pkgs.writeShellScript "record-wayland-gif.sh" ''
  set -u
  frames_dir="$1"
  output_gif="$2"
  stop_file="$3"
  log_file="$4"
  delay_cs="$5"
  interval="$6"

  rm -f "$stop_file" "$output_gif" "$log_file"
  rm -rf "$frames_dir"
  mkdir -p "$frames_dir"

  max_frames=450
  i=0
  while [ ! -f "$stop_file" ] && [ "$i" -lt "$max_frames" ]; do
    frame=$(printf "%s/frame-%04d.png" "$frames_dir" "$i")
    timeout 10 ${pkgs.grim}/bin/grim "$frame" >>"$log_file" 2>&1 || true
    i=$((i + 1))
    sleep "$interval"
  done

  if ls "$frames_dir"/frame-*.png >/dev/null 2>&1; then
    timeout 120 ${pkgs.imagemagick}/bin/convert -delay "$delay_cs" -loop 0 \
      "$frames_dir"/frame-*.png "$output_gif" >>"$log_file" 2>&1 || true
  fi
''
