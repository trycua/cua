#!/bin/bash
# node-stats.sh SECONDS OUT — every 5 s: cpu busy %, load1, mem used GiB, running containers, firecracker procs.
D=${1:-600}; OUT=${2:-$HOME/stats.csv}
echo "ts,cpu_busy_pct,load1,mem_used_gib,containers,microvms" > $OUT
END=$(( $(date +%s) + D ))
prev=$(awk '/^cpu /{print $2+$3+$4+$6+$7+$8, $2+$3+$4+$5+$6+$7+$8}' /proc/stat)
while [ "$(date +%s)" -lt "$END" ]; do
  sleep 5
  cur=$(awk '/^cpu /{print $2+$3+$4+$6+$7+$8, $2+$3+$4+$5+$6+$7+$8}' /proc/stat)
  busy=$(echo "$prev $cur" | awk '{db=$3-$1; dt=$4-$2; printf "%.1f", (dt>0?100*db/dt:0)}')
  prev=$cur
  mem=$(free -m | awk '/Mem:/{printf "%.1f", $3/1024}')
  c=$(sudo docker ps -q 2>/dev/null | wc -l)
  m=$(pgrep -c firecracker)
  echo "$(date +%s),$busy,$(cut -d' ' -f1 /proc/loadavg),$mem,$c,$m" >> $OUT
done
