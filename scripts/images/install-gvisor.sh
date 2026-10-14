#!/usr/bin/env bash
# Install gVisor's runsc (checksum-verified) and register it as a docker
# runtime on a CI runner. Idempotent. Needs sudo; never run on a laptop.
#
# GVISOR_VERSION pins the release (default below; "latest" follows upstream).
# Since 2026-09-23 releases ship one tarball per arch (gvisor.tar.bz2 with
# runsc, containerd-shim-runsc-v1 and gvisor-bin/) instead of bare runsc
# binaries; older pins still use the bare layout, so both are handled.
set -euo pipefail
if docker info --format '{{json .Runtimes}}' 2>/dev/null | grep -q '"runsc"'; then
    echo "runsc already registered"; exit 0
fi
version="${GVISOR_VERSION:-20260921.0}"
url="https://storage.googleapis.com/gvisor/releases/release/$version/$(uname -m)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
if curl -fsSL -o "$work/gvisor.tar.bz2" "$url/gvisor.tar.bz2"; then
    curl -fsSL -o "$work/gvisor.tar.bz2.sha512" "$url/gvisor.tar.bz2.sha512"
    (cd "$work" && sha512sum -c gvisor.tar.bz2.sha512)
    tar -xjf "$work/gvisor.tar.bz2" -C "$work" runsc containerd-shim-runsc-v1
else
    curl -fsSL -o "$work/runsc" "$url/runsc"
    curl -fsSL -o "$work/runsc.sha512" "$url/runsc.sha512"
    (cd "$work" && sha512sum -c runsc.sha512)
fi
sudo install -m 0755 "$work/runsc" /usr/local/bin/runsc
[ -f "$work/containerd-shim-runsc-v1" ] && sudo install -m 0755 "$work/containerd-shim-runsc-v1" /usr/local/bin/containerd-shim-runsc-v1
sudo /usr/local/bin/runsc install
sudo systemctl restart docker
/usr/local/bin/runsc --version | sed -n 1p
docker info --format '{{json .Runtimes}}'
