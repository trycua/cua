#!/bin/bash
# Run all 5 modality recordings against the Electron harness, save re-encoded
# h264 mp4s to ~/Desktop/cua-driver-modality-videos/linux-electron-<mode>.mp4.
rm -f /tmp/lin-electron-allmodes.log
OUT="$HOME/Desktop/cua-driver-modality-videos"; mkdir -p "$OUT"
for MODE in ax-fg ax-bg px-fg px-bg px-desktop; do
  bash /tmp/lin-run-electron.sh "$MODE" >/dev/null 2>&1
  echo "DONE $MODE $(cat /tmp/cua-lin-electron-$MODE/metric.log 2>/dev/null)" | tee -a /tmp/lin-electron-allmodes.log
  SRC="/tmp/cua-lin-electron-$MODE/rec/recording.mp4"
  [ -f "$SRC" ] && ffmpeg -y -loglevel error -i "$SRC" -c:v libx264 -crf 26 -pix_fmt yuv420p "$OUT/linux-electron-$MODE.mp4"
done
