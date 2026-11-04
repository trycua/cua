#!/usr/bin/env bash
# Offline test for check-tag-safety.sh with a fake crane (no network).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
cat >"$tmp/crane" <<'FAKE'
#!/usr/bin/env bash
case "$2" in
  *:existing|*:24.04|*:24.04-disk|*:24.04-slim|*:24.04-slim-disk|*:26|*:26-xcode|*:26-xcode-26.1|*:15-disk|*:2022-disk|*:main|*:24.04-full|*:26-xcode-beta|*:build-x|*-20260101-abcdef0|*-20260101-abcdef0-arm64|*:latest|*:1.0|*:1.0-disk|*:verified-v1|*:edge|*:edge-disk) echo sha256:aaa ;;
  *) echo "MANIFEST_UNKNOWN: manifest unknown" >&2; exit 1 ;;
esac
FAKE
chmod +x "$tmp/crane"
export CRANE="$tmp/crane"
pass=0 fail=0
check() { # expected-exit args...
    local want="$1"; shift; local rc=0
    "$HERE/check-tag-safety.sh" "$@" >/dev/null 2>&1 || rc=$?
    if [ "$rc" = "$want" ]; then pass=$((pass+1)); else fail=$((fail+1)); echo "FAIL (got $rc want $want): $*"; fi
}
check 0 ghcr.io/trycua/linux:24.04-20260923-86b137f          # new immutable
check 1 ghcr.io/trycua/linux:24.04                            # exists, not --moving
check 0 --moving ghcr.io/trycua/linux:24.04                   # canonical moving tag
check 1 --moving ghcr.io/trycua/linux:24.04-20260101-abcdef0  # immutable never moves
check 0 --digest sha256:aaa ghcr.io/trycua/linux:24.04-20260101-abcdef0  # same content
check 0 --moving ghcr.io/trycua/linux:24.04-disk              # canonical floating disk tag
check 1 --moving ghcr.io/trycua/linux:24.04-disk-20260101-abcdef0  # dated disk pin never moves
check 1 --moving ghcr.io/trycua/linux:docker-build-20260101-abcdef0-arm64  # per-arch child never moves
check 1 --moving ghcr.io/trycua/linux:build-x                 # canonical: only floating tags move
check 1 --moving ghcr.io/trycua/linux:latest                  # canonical: latest is not a floating version
check 0 --moving ghcr.io/trycua/macos:26                      # other canonical floats unchanged
check 0 --moving ghcr.io/trycua/windows:2022-disk
check 0 ghcr.io/trycua/linux:24.04-20260925-abcdef1-amd64     # new dated per-arch child
check 1 --moving ghcr.io/trycua/cua-desktop-linux:latest      # protected legacy
check 1 --moving public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:latest
check 1 --moving trycua/cua-qemu-linux:latest                 # docker.io short ref
check 0 ghcr.io/trycua/cua-desktop-linux:e2e-86b137f          # new unique tag in legacy repo
check 1 --moving ghcr.io/trycua/cua-plain-ubuntu-server:existing  # non-canonical moving
check 0 ghcr.io/trycua/linux@sha256:aaa
check 1 ghcr.io/trycua/linux                                  # no tag
# Canonical floating tags follow the tier grammar; latest is banned outright.
# (Repos in variables: these synthetic tags are tag-policy inputs, never pulled.)
L=ghcr.io/trycua/linux M=ghcr.io/trycua/macos W=ghcr.io/trycua/windows
check 0 --moving $L:24.04-slim
check 0 --moving $L:24.04-slim-disk
check 0 --moving $M:26
check 0 --moving $M:26-xcode
check 0 --moving $M:26-xcode-26.1
check 0 --moving $W:15-disk
check 1 --moving $L:main                   # not in the grammar
check 1 --moving $L:24.04-full             # full is the unsuffixed tag
check 1 --moving $M:26-xcode-beta          # not a version
check 1 --moving $L:latest                 # present, banned
check 1 $M:latest                          # absent, still banned
check 0 $L:24.04-slim-20260925-abcdef1     # new tier pin
check 0 $L:sha256-aaa                      # new referrer tag
# Benchmark repos: <ver> / <ver>-disk float; pins, latest and other repos do not.
check 0 ghcr.io/trycua/bench-web:1.0-20260923-abcdef1         # new immutable pin
check 1 ghcr.io/trycua/bench-web:1.0                          # exists, not --moving
check 0 --moving ghcr.io/trycua/bench-web:1.0                 # bench version tag
check 0 --moving ghcr.io/trycua/bench-web:1.0-disk            # bench disk version tag
check 0 --moving ghcr.io/trycua/bench-osworld:verified-v1
check 1 --moving ghcr.io/trycua/bench-web:1.0-20260101-abcdef0   # immutable never moves
check 1 --moving ghcr.io/trycua/bench-web:latest              # reserved on bench repos
check 1 --moving ghcr.io/trycua/benchmarks:1.0                # not a bench-* repo
check 1 --moving ghcr.io/trycua/cua-desktop-linux:1.0         # protected legacy repo
# Channel repos: edge / edge-disk (and rc, stable) float; pins and others do not.
check 0 ghcr.io/trycua/omarchy:edge-20260925-abcdef1          # new immutable pin
check 1 ghcr.io/trycua/omarchy:edge                           # exists, not --moving
check 0 --moving ghcr.io/trycua/omarchy:edge                  # channel tag
check 0 --moving ghcr.io/trycua/omarchy:edge-disk             # channel disk tag
check 1 --moving ghcr.io/trycua/omarchy:edge-disk-20260101-abcdef0  # immutable never moves
check 1 --moving ghcr.io/trycua/omarchy:latest                # not a channel tag
echo "pass=$pass fail=$fail"; [ "$fail" = 0 ]
