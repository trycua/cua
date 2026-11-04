#!/usr/bin/env bash
# Host tools `cua images release` steps use, on a GitHub-hosted Ubuntu runner
# (what the image workflows run before `cua images release`). Locally, install
# the same with your package manager: docker (with runsc registered for the
# gVisor lanes), qemu (qemu-img, qemu-system-*), UEFI firmware, genisoimage,
# virtiofsd, zstd, crane, oras, jq, python3.
#
#   install-release-tools.sh [--push-only] [--free-disk]
#
# --push-only: just zstd, crane and oras (push, attest, publish, verify jobs).
# --free-disk: also remove preinstalled toolchains the image builds never use.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PUSH_ONLY=0 FREE=0
for a in "$@"; do
    case "$a" in
        --push-only) PUSH_ONLY=1 ;;
        --free-disk) FREE=1 ;;
        *) echo "unknown option $a" >&2; exit 2 ;;
    esac
done
arch="$(dpkg --print-architecture)"
pkgs=(zstd jq)
if [ "$PUSH_ONLY" = 0 ]; then
    fw=ovmf; [ "$arch" = arm64 ] && fw=qemu-efi-aarch64
    pkgs+=(qemu-utils qemu-system-x86 qemu-system-arm "$fw" genisoimage virtiofsd skopeo)
fi
sudo apt-get update -qq
sudo apt-get install -y -qq --no-install-recommends "${pkgs[@]}"
gobin="$(go env GOPATH)/bin"
command -v crane >/dev/null || go install github.com/google/go-containerregistry/cmd/crane@v0.20.6
command -v oras >/dev/null || go install oras.land/oras/cmd/oras@v1.3.0
[ -n "${GITHUB_PATH:-}" ] && echo "$gobin" >>"$GITHUB_PATH"
export PATH="$gobin:$PATH"
# GITHUB_PATH reaches only later steps, and this PATH only this script: a
# step that installs and then runs a release in the same shell needs the
# tools on the default PATH too.
for tool in crane oras; do
    [ -x "/usr/local/bin/$tool" ] || [ ! -x "$gobin/$tool" ] || sudo ln -sf "$gobin/$tool" "/usr/local/bin/$tool"
done
if [ "$PUSH_ONLY" = 0 ]; then
    # KVM for the VM lanes when the runner exposes it; gVisor as a docker runtime.
    if [ -e /dev/kvm ]; then sudo chmod 666 /dev/kvm; fi
    "$HERE/install-gvisor.sh"
fi
if [ "$FREE" = 1 ]; then
    # Not /opt/hostedtoolcache as a whole: setup-uv's uv lives there.
    sudo rm -rf /usr/share/dotnet /usr/local/lib/android /opt/ghc /usr/local/share/boost /usr/local/.ghcup \
        /opt/hostedtoolcache/CodeQL /opt/hostedtoolcache/PyPy /opt/hostedtoolcache/Ruby /opt/hostedtoolcache/Java_* \
        /usr/share/swift /usr/lib/jvm /usr/local/share/powershell /usr/local/share/chromium /usr/local/share/vcpkg \
        /usr/share/miniconda /opt/az /opt/microsoft /opt/google /usr/local/julia* /usr/share/kotlinc || true
    sudo docker image prune -af >/dev/null 2>&1 || true
fi
df -h /
