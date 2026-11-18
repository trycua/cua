#!/usr/bin/env bash
# The docker tier's in-guest smoke test (ghcr.io/trycua/linux:24.04-docker-disk).
# Runs as root inside a booted VM of the tier; smoke-docker-vm.sh boots the
# disk under QEMU and pipes this in over ssh, and the macOS integration lane
# runs it in a real `vm` Space. Checks, in order:
#   1. dockerd answers (docker-ready), and how long after boot it did
#   2. overlay and br_netfilter are loaded at boot (not by this script)
#   3. iptables and ip6tables (nft backend): a filter chain, a nat REDIRECT
#      rule and a physdev match, the rules Docker Compose egress filters use
#   4. a compose hello-world: a healthchecked web service on an IPv4+IPv6
#      bridge network, `docker compose up --wait`, its published port served,
#      the container's IPv6 address, `docker exec`, then `compose down`
# Prints one line `RESULT {json}` with the timings; exits non-zero on the
# first failed check.
set -euo pipefail
BUSYBOX="${CUA_SMOKE_BUSYBOX:-mirror.gcr.io/library/busybox:1.37@sha256:bdf57e528e45e4433820e045b29b4597825a1c9e38353532d90a01445013f82e}"
ok() { echo "  ok   $*"; }
fail() { echo "  FAIL $*" >&2; exit 1; }
now() { date +%s.%N; }
since() { awk -v a="$1" -v b="$(now)" 'BEGIN{printf "%.2f", b-a}'; }

[ "$(id -u)" = 0 ] || fail "run as root"
[ "$(cat /etc/cua-image/variant 2>/dev/null)" = vm ] || fail "not the VM variant of the image"

# 1. dockerd
ready_line="$(docker-ready 180)" || fail "dockerd not ready"
dockerd_ready_s="$(echo "$ready_line" | awk '{print $3}' | tr -d s)"
ok "$ready_line"
systemctl is-enabled --quiet docker.service || fail "docker.service is not enabled"
systemctl is-active --quiet containerd.service || fail "containerd.service is not active"
server="$(docker version --format '{{.Server.Version}}')"
driver="$(docker info --format '{{.Driver}}')"
cgroup="$(docker info --format '{{.CgroupVersion}}')"
ok "docker $server, storage $driver, cgroup v$cgroup"
[ "$cgroup" = 2 ] || fail "cgroup v$cgroup (want 2)"

# 2. modules loaded at boot
for m in overlay br_netfilter; do
    [ -d "/sys/module/$m" ] || fail "module $m not loaded"
done
systemctl show -p ActiveState --value systemd-modules-load.service | grep -qx active \
    || fail "systemd-modules-load did not run"
[ -e /proc/sys/net/bridge/bridge-nf-call-iptables ] || fail "no bridge-nf-call-iptables"
modprobe br_netfilter || fail "modprobe br_netfilter"
ok "overlay and br_netfilter loaded at boot; modprobe br_netfilter works"

# 3. iptables / ip6tables
iptables --version | grep -q nf_tables || fail "iptables is not the nft backend"
chain=CUA_SMOKE
cleanup_rules() {
    iptables -t nat -D PREROUTING -m physdev --physdev-in cua-smoke0 -j "$chain" 2>/dev/null || true
    iptables -D FORWARD -m physdev --physdev-in cua-smoke0 -j "$chain" 2>/dev/null || true
    ip6tables -D FORWARD -m physdev --physdev-in cua-smoke0 -j "$chain" 2>/dev/null || true
    for t in "iptables -t nat" iptables ip6tables; do $t -F "$chain" 2>/dev/null || true; $t -X "$chain" 2>/dev/null || true; done
}
trap cleanup_rules EXIT
iptables -t nat -N "$chain"
iptables -N "$chain"
ip6tables -N "$chain"
iptables -t nat -A "$chain" -p tcp -j REDIRECT --to-ports 3128
iptables -A "$chain" -p udp -j REJECT
ip6tables -A "$chain" -j REJECT
iptables -t nat -I PREROUTING 1 -m physdev --physdev-in cua-smoke0 -j "$chain"
iptables -I FORWARD 1 -m physdev --physdev-in cua-smoke0 -j "$chain"
ip6tables -I FORWARD 1 -m physdev --physdev-in cua-smoke0 -j "$chain"
iptables -t nat -S PREROUTING | grep -q -- "--physdev-in cua-smoke0" || fail "physdev rule missing"
cleanup_rules
trap - EXIT
ok "iptables/ip6tables: filter, nat REDIRECT and physdev rules"

# 4. compose hello-world
work="$(mktemp -d)"
trap 'docker compose -p cua-smoke -f "$work/compose.yaml" down -v --timeout 2 >/dev/null 2>&1 || true; rm -rf "$work"' EXIT
cat >"$work/compose.yaml" <<YAML
services:
  web:
    image: $BUSYBOX
    command: ["sh", "-c", "mkdir -p /www && echo cua-docker-smoke >/www/index.html && exec httpd -f -p 8080 -h /www"]
    ports: ["127.0.0.1:18081:8080"]
    networks: [world]
    healthcheck:
      test: ["CMD", "wget", "-q", "-O", "/dev/null", "http://127.0.0.1:8080/"]
      interval: 1s
      timeout: 2s
      retries: 30
networks:
  world:
    enable_ipv6: true
    ipam:
      config:
        - subnet: 172.31.250.0/24
        - subnet: fd00:c0a:5::/64
YAML
t="$(now)"
docker compose -p cua-smoke -f "$work/compose.yaml" pull --quiet
pull_s="$(since "$t")"
t="$(now)"
docker compose -p cua-smoke -f "$work/compose.yaml" up -d --wait --wait-timeout 120 >/dev/null
up_s="$(since "$t")"
ok "compose pull ${pull_s}s, up --wait ${up_s}s"
[ "$(curl -fsS --max-time 5 http://127.0.0.1:18081/)" = cua-docker-smoke ] || fail "published port did not serve"
cid="$(docker compose -p cua-smoke -f "$work/compose.yaml" ps -q web)"
v6="$(docker inspect --format '{{range .NetworkSettings.Networks}}{{.GlobalIPv6Address}}{{end}}' "$cid")"
[ -n "$v6" ] || fail "web has no IPv6 address"
[ "$(docker exec "$cid" cat /www/index.html)" = cua-docker-smoke ] || fail "docker exec"
ok "served on its published port; IPv6 $v6; docker exec works"
docker compose -p cua-smoke -f "$work/compose.yaml" down -v --timeout 2 >/dev/null
echo "RESULT {\"dockerd_ready_s\": $dockerd_ready_s, \"docker\": \"$server\", \"storage_driver\": \"$driver\", \"compose_pull_s\": $pull_s, \"compose_up_s\": $up_s}"
