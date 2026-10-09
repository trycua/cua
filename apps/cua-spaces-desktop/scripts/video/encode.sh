#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Synthetic H.264 streams for the video harnesses
# (apps/cua-spaces-macos/scripts/native-video-harness.sh).
#
# Each output is a raw Annex B elementary stream shaped like what the Space
# host encoder sends on rcdp wire v2: no B-frames, SPS/PPS repeated before
# every IDR, one access unit per frame. The picture is a still test card
# (or SRC=<video>, any recording of a desktop) with a moving testsrc2 panel
# on top, so every frame changes somewhere, like a desktop with an animation
# running.
#
#   ./encode.sh            # writes .cache/{tile30,tile60,tile720p60,full60,tile10}.h264
#
# tile10 is what a Space tile asks for in production (VideoTier.tile in the
# macOS app: 10 fps, 960 px long edge).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
out="$here/.cache"
mkdir -p "$out"

encode() { # name width height fps bitrate
  local name=$1 w=$2 h=$3 fps=$4 rate=$5
  local pw=$((w / 4)) ph=$((h / 4))
  local src
  if [ -n "${SRC:-}" ]; then
    src=(-ss 4 -t 30 -i "$SRC")
  else
    src=(-f lavfi -t 30 -i "smptehdbars=size=${w}x${h}:rate=${fps}")
  fi
  ffmpeg -hide_banner -loglevel error -y \
    "${src[@]}" \
    -f lavfi -i "testsrc2=size=${pw}x${ph}:rate=${fps}" \
    -filter_complex "[0:v]fps=${fps},scale=${w}:${h}:flags=bicubic[bg];[bg][1:v]overlay=x='(W-w)*(0.5+0.45*sin(t*0.9))':y='(H-h)*(0.5+0.45*cos(t*0.7))':shortest=1,format=yuv420p" \
    -c:v libx264 -preset medium -tune zerolatency -profile:v high -bf 0 \
    -b:v "$rate" -maxrate "$rate" -bufsize "$rate" \
    -g $((fps * 10)) -keyint_min $((fps * 10)) -sc_threshold 0 \
    -x264-params "repeat-headers=1:aud=0" \
    -bsf:v h264_mp4toannexb -f h264 "$out/$name.h264"
  echo "$name: $(du -h "$out/$name.h264" | cut -f1)"
}

encode tile30 1280 800 30 4M
encode tile60 1280 800 60 6M
encode tile720p60 1280 720 60 6M
encode full60 1920 1080 60 10M
encode tile10 960 600 10 1M
