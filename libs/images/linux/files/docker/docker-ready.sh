#!/usr/bin/env bash
# docker-ready [TIMEOUT_S]: wait until dockerd answers on its socket (the
# docker tier's readiness check; `cua sb bake` and the image smoke test run
# it). Prints when dockerd became ready, in seconds since the kernel booted:
# docker.service's own ready time (systemd's monotonic ActiveEnter
# timestamp), else the moment this check first got an answer.
set -euo pipefail
timeout="${1:-120}"
deadline=$((SECONDS + timeout))
until docker version --format '{{.Server.Version}}' >/dev/null 2>&1; do
    if [ "$SECONDS" -ge "$deadline" ]; then
        echo "docker-ready: dockerd did not answer within ${timeout}s" >&2
        systemctl --no-pager status docker.service containerd.service >&2 2>/dev/null || true
        exit 1
    fi
    sleep 0.1
done
usec="$(systemctl show -p ActiveEnterTimestampMonotonic --value docker.service 2>/dev/null || echo 0)"
if [ "${usec:-0}" -gt 0 ] 2>/dev/null; then
    awk -v u="$usec" 'BEGIN{printf "dockerd ready %.2fs after boot\n", u/1e6}'
else
    awk '{printf "dockerd ready %.2fs after boot\n", $1}' /proc/uptime
fi
