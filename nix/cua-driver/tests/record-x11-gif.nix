# Shared X11 screen-recording helper for the Linux visual (GIF) tests.
#
# Returns a `pkgs.writeShellScript` that loops `import -display <display>
# -window root frame-XXXX.png` until a stop-file appears, then stitches the
# frames into an animated GIF with `convert`. Both tools come from
# `pkgs.imagemagick`, which the importing test must add to
# `environment.systemPackages`.
#
# Usage (in a test's `let`):
#   recordGifScript = import ./record-x11-gif.nix { inherit pkgs; };
# then, inside the testScript, start it in the background before driving and
# `touch` the stop-file afterwards:
#   ${recordGifScript} <display> <frames_dir> <output_gif> <stop_file> \
#                       <log_file> <delay_cs> <interval>
{ pkgs }:

pkgs.writeShellScript "record-x11-gif.sh" ''
  set -eu
  display="$1"
  frames_dir="$2"
  output_gif="$3"
  stop_file="$4"
  log_file="$5"
  delay_cs="$6"
  interval="$7"

  rm -f "$stop_file" "$output_gif" "$log_file"
  rm -rf "$frames_dir"
  mkdir -p "$frames_dir"

  i=0
  while [ ! -f "$stop_file" ]; do
    frame=$(printf "%s/frame-%04d.png" "$frames_dir" "$i")
    import -display "$display" -window root "$frame" >>"$log_file" 2>&1 || true
    i=$((i + 1))
    sleep "$interval"
  done

  if ls "$frames_dir"/frame-*.png >/dev/null 2>&1; then
    convert -delay "$delay_cs" -loop 0 "$frames_dir"/frame-*.png "$output_gif" >>"$log_file" 2>&1
  fi
''
