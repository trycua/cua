#!/usr/bin/env bash
# Headless PipeWire for the desktop user: pipewire + wireplumber +
# pipewire-pulse (Pulse protocol at $XDG_RUNTIME_DIR/pulse/native), with the
# virtual devices from /etc/pipewire/pipewire.conf.d/10-cua-virtual-devices.conf
# made the defaults. Runs under supervisord (container) and systemd (VM); exits
# non-zero if any of the three dies so the supervisor restarts the set.
set -euo pipefail
log() { echo "[$(date -Iseconds)] start-audio: $*"; }
export HOME="${HOME:-/home/$(id -un)}"
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/cua-desktop}"

# Share the desktop session bus when it is up (wireplumber uses it for
# reservation/portal features; audio works without it).
ENV_FILE="$XDG_RUNTIME_DIR/desktop.env"
for _ in $(seq 1 30); do [ -r "$ENV_FILE" ] && break; sleep 1; done
if [ -r "$ENV_FILE" ]; then set -a; . "$ENV_FILE"; set +a; fi
unset DISPLAY  # audio daemons don't need X

rm -f "$XDG_RUNTIME_DIR/pipewire-0" "$XDG_RUNTIME_DIR/pipewire-0.lock" "$XDG_RUNTIME_DIR/pipewire-0-manager"*
PIDS=()
cleanup() { for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null || true; done; wait 2>/dev/null || true; }
trap cleanup EXIT TERM INT

pipewire &
PIDS+=($!)
for _ in $(seq 1 50); do [ -S "$XDG_RUNTIME_DIR/pipewire-0" ] && break; sleep 0.1; done
[ -S "$XDG_RUNTIME_DIR/pipewire-0" ] || { log "pipewire did not create its socket"; exit 1; }
wireplumber &
PIDS+=($!)
pipewire-pulse &
PIDS+=($!)

for _ in $(seq 1 100); do pactl info >/dev/null 2>&1 && break; sleep 0.1; done
for _ in $(seq 1 50); do pactl list short sinks 2>/dev/null | grep -q cua_desktop && break; sleep 0.1; done
pactl set-default-sink cua_desktop || log "could not set default sink"
for _ in $(seq 1 50); do pactl list short sources 2>/dev/null | grep -q "\bcua_mic\b" && break; sleep 0.1; done
pactl set-default-source cua_mic || log "could not set default source"
log "audio up: $(pactl info 2>/dev/null | grep -E '^(Server Name|Default Sink|Default Source)' | tr '\n' ';')"

wait -n "${PIDS[@]}"
log "an audio daemon exited; tearing down"
exit 1
