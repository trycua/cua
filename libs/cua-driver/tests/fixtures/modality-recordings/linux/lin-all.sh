#!/bin/bash
rm -f /tmp/lin-allmodes.log
for MODE in ax-fg ax-bg px-fg px-bg px-desktop; do
  bash /tmp/lin-run.sh "$MODE" >/dev/null 2>&1
  echo "DONE $MODE $(cat /tmp/cua-lin-$MODE/metric.log 2>/dev/null)" | tee -a /tmp/lin-allmodes.log
done
