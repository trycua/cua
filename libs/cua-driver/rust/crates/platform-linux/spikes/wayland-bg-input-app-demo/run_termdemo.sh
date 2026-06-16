#!/bin/bash
cd /opt/spike
rm -f mycalc /tmp/calc_coords.txt
foot --working-directory=/opt/spike --window-size-chars=92x26 >/tmp/foot.log 2>&1 &
sleep 4
wf-recorder -c libvpx -r 25 -f /tmp/demo_term.webm >/tmp/wf.log 2>&1 &
WF=$!
sleep 1
stdbuf -oL ./type_use >/tmp/typeuse.log 2>&1 &
sleep 24
kill -INT $WF
sleep 3
