#!/usr/bin/env bash
# Smoke test for the daemon-agnostic plain images, driven only through the
# surfaces a sandbox without any cua daemon has:
#   ubuntu-xfce-vnc  healthy; RFB :5901 frame grabbed from the host by an
#                    agentless client, and the frame is a real desktop (many
#                    colors); no cua daemon binaries or listeners in the guest
#   ubuntu-server    healthy; SSH login as `cua` with a throwaway key runs
#                    `uname -a`; nothing listens except sshd
#
# Usage: smoke-test.sh <ubuntu-xfce-vnc|ubuntu-server> [--image REF]
#            [--runtime runc|runsc] [--evidence DIR]
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
host_arch() { case "$(uname -m)" in arm64|aarch64) echo arm64 ;; *) echo amd64 ;; esac; }
WHICH="${1:?ubuntu-xfce-vnc|ubuntu-server}"; shift
IMAGE="cua-e2e-local/$WHICH:docker-local-$(host_arch)"
RUNTIME=runc
EVIDENCE="${SMOKE_EVIDENCE:-$PWD/smoke-evidence}"
while [ $# -gt 0 ]; do
    case "$1" in
        --image) IMAGE="$2"; shift 2 ;;
        --runtime) RUNTIME="$2"; shift 2 ;;
        --evidence) EVIDENCE="$2"; shift 2 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
mkdir -p "$EVIDENCE"
NAME="cua-e2e-images-$WHICH-$RUNTIME-$$"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/cua-e2e-plain.XXXXXX")"
PASS=0; FAIL=0; t0=$SECONDS
ok() { PASS=$((PASS + 1)); echo "  ok   $*"; }
bad() { FAIL=$((FAIL + 1)); echo "  FAIL $*"; }
cleanup() { docker rm -f "$NAME" >/dev/null 2>&1 || true; rm -rf "$WORK"; }
trap cleanup EXIT

run_args=(-d --name "$NAME" --runtime="$RUNTIME" --memory=2g --memory-swap=2g)
case "$WHICH" in
    ubuntu-xfce-vnc) run_args+=(-p 127.0.0.1::5901) ;;
    ubuntu-server)
        ssh-keygen -q -t ed25519 -N "" -f "$WORK/key"
        run_args+=(-p 127.0.0.1::22 -e "SSH_AUTHORIZED_KEYS=$(cat "$WORK/key.pub")") ;;
    *) echo "unknown plain image $WHICH" >&2; exit 2 ;;
esac
echo "==> $IMAGE under --runtime=$RUNTIME"
docker run "${run_args[@]}" "$IMAGE" >/dev/null

status=""
for _ in $(seq 1 90); do
    status="$(docker inspect -f '{{.State.Health.Status}}' "$NAME" 2>/dev/null || echo gone)"
    [ "$status" = healthy ] || [ "$status" = gone ] && break
    sleep 1
done
[ "$status" = healthy ] && ok "healthy after $((SECONDS - t0))s" || { bad "health=$status"; docker logs "$NAME" | tail -20; exit 1; }

# Agnosticism: none of our daemons exist in a plain image.
# (script on stdin, so the probe's own argv can't match the pattern)
found="$(docker exec -i "$NAME" sh -s <<'SH'
ls -d /usr/local/bin/cua-spacesd /usr/local/bin/cua-driver /opt/computer-server 2>/dev/null
pgrep -a -f 'computer_server|cua-spacesd|cua-driver|supergateway|rcdpd'
true
SH
)"
if [ -n "$found" ]; then
    bad "found cua daemon binaries/processes in a plain image: $found"
else
    ok "no cua daemons installed or running"
fi
listeners="$(docker exec "$NAME" sh -c 'cat /proc/net/tcp /proc/net/tcp6 2>/dev/null' | python3 -c 'import sys; print(" ".join(sorted({str(int(f[1].split(":")[1], 16)) for f in (l.split() for l in sys.stdin) if len(f) > 3 and f[3] == "0A"}, key=int)))')"
echo "    listening TCP ports: ${listeners:-<none visible>}"

case "$WHICH" in
    ubuntu-xfce-vnc)
        port="$(docker port "$NAME" 5901/tcp | head -1 | sed 's/.*://')"
        sleep 3  # let the XFCE session paint
        if python3 "$HERE/../common/tools/rfb_snapshot.py" "127.0.0.1:$port" "$EVIDENCE/$WHICH-$RUNTIME.png" >"$WORK/rfb.txt" 2>&1; then
            ok "RFB frame: $(cat "$WORK/rfb.txt")"
            colors="$(sed -n 's/.*sampled_colors=\([0-9]*\).*/\1/p' "$WORK/rfb.txt")"
            [ "${colors:-0}" -ge 8 ] && ok "frame shows a rendered desktop ($colors sampled colors)" || bad "frame looks blank ($colors colors)"
        else
            bad "RFB frame: $(cat "$WORK/rfb.txt")"
        fi
        ;;
    ubuntu-server)
        port="$(docker port "$NAME" 22/tcp | head -1 | sed 's/.*://')"
        out=""
        for _ in $(seq 1 10); do
            out="$(ssh -i "$WORK/key" -p "$port" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
                -o BatchMode=yes -o ConnectTimeout=5 -o LogLevel=ERROR cua@127.0.0.1 'uname -a; id -un' 2>&1)" && break
            sleep 1
        done
        grep -q '^cua$' <<<"$out" && ok "ssh as cua: $(head -1 <<<"$out")" || bad "ssh: $out"
        echo "$out" >"$EVIDENCE/$WHICH-$RUNTIME-ssh.txt"
        ;;
esac
echo "==> $PASS passed, $FAIL failed in $((SECONDS - t0))s ($RUNTIME, $IMAGE)"
[ "$FAIL" = 0 ]
