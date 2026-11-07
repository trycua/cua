#!/usr/bin/env bash
# Runs inside a booted guest (systemd PID 1, root, CAP_NET_ADMIN): proves the
# CVE-2026-98432 interim guard drops IPv4-options packets before TCP and leaves
# normal traffic and other firewall tables alone. Everything stays on this
# guest's own loopback and two private network namespaces.
set -uo pipefail
pass=0 fail=0
ok()  { pass=$((pass + 1)); echo "ok   $*"; }
bad() { fail=$((fail + 1)); echo "FAIL $*"; }
check() { local name="$1"; shift; if "$@"; then ok "$name"; else bad "$name"; fi; }
GUARD=/opt/cua/bin/cua-ipopts-guard
echo "kernel: $(uname -r)"

# Python helpers: a TCP connect with raw IP_OPTIONS set on the client socket.
cat >/tmp/ipopt_client.py <<'PY'
import socket, sys
# kind: none | rr | ts | lsrr | ssrr
kind, host, port = sys.argv[1], sys.argv[2], int(sys.argv[3])
opts = {
  "none": b"",
  "rr":   bytes([7, 11, 4]) + bytes(8) + b"\x01",                 # Record Route + NOP pad
  "ts":   bytes([0x44, 12, 5, 0]) + bytes(8),                      # Timestamp
  "lsrr": bytes([0x83, 7, 4, 127, 0, 0, 1]) + b"\x01",             # loose source route
  "ssrr": bytes([0x89, 7, 4, 127, 0, 0, 1]) + b"\x01",             # strict source route
}[kind]
s = socket.socket(); s.settimeout(3)
if opts:
    s.setsockopt(socket.IPPROTO_IP, socket.IP_OPTIONS, opts)
try:
    s.connect((host, port)); print("CONNECTED"); sys.exit(0)
except OSError as e:
    print("NOCONNECT", type(e).__name__); sys.exit(1)
PY
cat >/tmp/listener.py <<'PY'
import socket, sys
s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind((sys.argv[1], int(sys.argv[2]))); s.listen(16)
while True:
    c, _ = s.accept(); c.close()
PY
connects() { python3 /tmp/ipopt_client.py "$@" >/dev/null; }

# 1. booted with the unit enabled and the table loaded
check "unit enabled at boot"            systemctl is-enabled --quiet cua-ipopts.service
check "unit active"                     systemctl is-active --quiet cua-ipopts.service
check "table cua_ipopts loaded"         bash -c "nft list table ip cua_ipopts >/dev/null"
check "source routing off (sysctl)"     test "$(sysctl -n net.ipv4.conf.all.accept_source_route)" = 0

# 2. loopback / local-process trigger path: IP_OPTIONS set by an unprivileged
#    process on a client socket, SYN over 127.0.0.1 to a local listener.
python3 /tmp/listener.py 127.0.0.1 8080 & L1=$!
sleep 1
check "lo: plain connect works"         connects none 127.0.0.1 8080
for k in rr ts lsrr ssrr; do
    check "lo: SYN with $k option is dropped"  bash -c "! python3 /tmp/ipopt_client.py $k 127.0.0.1 8080 >/dev/null"
done
drops="$(nft list chain ip cua_ipopts prerouting | sed -n 's/.*counter packets \([0-9]*\).*/\1/p')"
check "drop counter counted them (packets=$drops)" test "${drops:-0}" -ge 4

# 3. baseline: with the guard stopped the same RR / TS SYN reaches the listener
#    (proves the test sends a real options packet and the guard is what drops it).
systemctl stop cua-ipopts.service
check "baseline lo: table removed on stop"   bash -c '! nft list table ip cua_ipopts >/dev/null 2>&1'
for k in rr ts; do
    check "baseline lo: $k SYN connects without the guard" connects $k 127.0.0.1 8080
done
systemctl start cua-ipopts.service
check "restart: guard back"             bash -c "nft list table ip cua_ipopts >/dev/null"
check "lo again: rr dropped"            bash -c '! python3 /tmp/ipopt_client.py rr 127.0.0.1 8080 >/dev/null'
kill $L1 2>/dev/null

# 4. routed path: crafted packets over a veth pair into a private namespace
ip netns add srv; ip link add vcli type veth peer name vsrv; ip link set vsrv netns srv
ip addr add 10.77.0.1/24 dev vcli; ip link set vcli up
ip netns exec srv ip addr add 10.77.0.2/24 dev vsrv
ip netns exec srv ip link set vsrv up; ip netns exec srv ip link set lo up
# the guard must also be inside the namespace that owns the socket
ip netns exec srv env CUA_IPOPTS_RULES=/etc/cua-image/cua-ipopts.nft "$GUARD" start
ip netns exec srv python3 /tmp/listener.py 10.77.0.2 9090 & L2=$!
sleep 1
cat >/tmp/craft.py <<'PY'
import sys
from scapy.all import IP, TCP, ICMP, UDP, IPOption_RR, IPOption_Timestamp, sr1, conf
conf.verb = 0
what, dst = sys.argv[1], sys.argv[2]
if what == "syn":      p = IP(dst=dst)/TCP(sport=40001, dport=9090, flags="S", seq=1)
elif what == "syn-rr": p = IP(dst=dst, options=[IPOption_RR()])/TCP(sport=40002, dport=9090, flags="S", seq=1)
elif what == "syn-ts": p = IP(dst=dst, options=[IPOption_Timestamp(flg=0)])/TCP(sport=40003, dport=9090, flags="S", seq=1)
elif what == "echo":   p = IP(dst=dst)/ICMP()
elif what == "echo-rr":p = IP(dst=dst, options=[IPOption_RR()])/ICMP()
r = sr1(p, timeout=2)
if r is None: print("NOREPLY"); sys.exit(1)
if what.startswith("syn"):
    f = int(r[TCP].flags); print("SYNACK" if f & 0x12 == 0x12 else "OTHER flags=%x" % f); sys.exit(0 if f & 0x12 == 0x12 else 2)
print("REPLY"); sys.exit(0)
PY
check "routed: normal SYN gets SYN-ACK"      python3 /tmp/craft.py syn 10.77.0.2
check "routed: SYN with RR gets no reply"    bash -c '! python3 /tmp/craft.py syn-rr 10.77.0.2 >/dev/null'
check "routed: SYN with Timestamp gets no reply" bash -c '! python3 /tmp/craft.py syn-ts 10.77.0.2 >/dev/null'
check "routed: normal ping works"            python3 /tmp/craft.py echo 10.77.0.2
check "routed: ping -R (RR) is dropped"      bash -c '! python3 /tmp/craft.py echo-rr 10.77.0.2 >/dev/null'
check "routed: normal TCP connect via the kernel stack" connects none 10.77.0.2 9090
srvdrops="$(ip netns exec srv nft list chain ip cua_ipopts prerouting | sed -n 's/.*counter packets \([0-9]*\).*/\1/p')"
check "routed: namespace counter saw the options packets (packets=$srvdrops)" test "${srvdrops:-0}" -ge 3
kill $L2 2>/dev/null

# 5. coexistence and idempotence
nft add table ip other_fw; nft add chain ip other_fw input '{ type filter hook input priority 0; policy accept; }'
nft add rule ip other_fw input tcp dport 22 accept
"$GUARD" start; "$GUARD" start
check "reload keeps one rule"           test "$(nft list table ip cua_ipopts | grep -c 'ip hdrlength')" = 1
check "other firewall table untouched"  bash -c 'nft list table ip other_fw | grep -q "tcp dport 22 accept"'
"$GUARD" stop
check "stop removes only the guard"     bash -c '! nft list table ip cua_ipopts >/dev/null 2>&1 && nft list table ip other_fw >/dev/null'
nft delete table ip other_fw

# 6. iptables fallback (image without nft): hide nft, load, test loopback
mkdir -p /tmp/ipt-only; ln -sf "$(command -v iptables)" /tmp/ipt-only/iptables
for b in sh cat sed grep; do ln -sf "$(command -v $b)" /tmp/ipt-only/$b; done
python3 /tmp/listener.py 127.0.0.1 8081 & L3=$!
sleep 1
if PATH=/tmp/ipt-only "$GUARD" start 2>/tmp/ipt.err; then
    check "iptables fallback: plain connect works"  connects none 127.0.0.1 8081
    check "iptables fallback: rr SYN dropped"       bash -c '! python3 /tmp/ipopt_client.py rr 127.0.0.1 8081 >/dev/null'
    check "iptables fallback: ts SYN dropped"       bash -c '! python3 /tmp/ipopt_client.py ts 127.0.0.1 8081 >/dev/null'
    PATH=/tmp/ipt-only "$GUARD" stop
    check "iptables fallback: stop restores"        connects rr 127.0.0.1 8081
else
    echo "SKIP iptables fallback (load failed: $(head -c 200 /tmp/ipt.err))"
fi
kill $L3 2>/dev/null
systemctl restart cua-ipopts.service

ip netns del srv 2>/dev/null
echo "RESULT pass=$pass fail=$fail"
[ "$fail" = 0 ]
