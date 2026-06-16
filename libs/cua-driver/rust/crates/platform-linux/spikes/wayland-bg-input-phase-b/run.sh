#!/bin/bash
# Phase B end-to-end: build the multi-cursor EIS-server compositor + cursive
# libei client, drive N background windows, render the montage.
# Prereqs: ../wayland-bg-input-phase-a/provision.sh + python3-pil fonts-dejavu-core.
set -e
HERE="$(cd "$(dirname "$0")" && pwd)"
WORK=/opt/spike; mkdir -p "$WORK"; cd "$WORK"
NDEV="${CUA_NDEV:-16}"

curl -fsSL 'https://gitlab.freedesktop.org/wlroots/wlroots/-/raw/0.19/tinywl/tinywl.c' -o tinywl.c
cp "$HERE/phase_b_patch.py" "$HERE/ei_client_b.c" "$HERE/render_demo.py" .
python3 phase_b_patch.py
wayland-scanner server-header /usr/share/wayland-protocols/stable/xdg-shell/xdg-shell.xml xdg-shell-protocol.h
wayland-scanner private-code  /usr/share/wayland-protocols/stable/xdg-shell/xdg-shell.xml xdg-shell-protocol.c
gcc -DWLR_USE_UNSTABLE -I. tinywl.c xdg-shell-protocol.c -o tinywl_b \
    $(pkg-config --cflags --libs wlroots-0.19 wayland-server xkbcommon pixman-1 libeis-1.0)
gcc ei_client_b.c -o ei_client_b $(pkg-config --cflags --libs libei-1.0) -lm

export XDG_RUNTIME_DIR=/tmp/spike-xdg; mkdir -p "$XDG_RUNTIME_DIR"; chmod 700 "$XDG_RUNTIME_DIR"
export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_HEADLESS_OUTPUTS=1
export CUA_EIS_SOCKET="$XDG_RUNTIME_DIR/cua-eis-0"; rm -f "$CUA_EIS_SOCKET"
export CUA_NDEV="$NDEV"
rm -f /tmp/ink-*.log
timeout 24 ./tinywl_b -s "for i in \$(seq 0 $((NDEV-1))); do stdbuf -oL wev >/tmp/ink-\$i.log 2>&1 & done; sleep 9; stdbuf -oL ./ei_client_b >/tmp/eib.log 2>&1 &" >/tmp/tinywl_b.log 2>&1 || true
python3 render_demo.py
echo "done -> /tmp/cursive_montage.png"
