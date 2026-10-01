#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
#
# The Cua Volume e2e (tests/volume_e2e.rs) against real Spaces:
#
#   run-volume-e2e.sh             a Linux Space (Docker; gVisor when the
#                                 engine has it)
#   run-volume-e2e.sh --macos     also a macOS Space (Lume, 6 GiB), from
#                                 CUA_VOLUME_E2E_MACOS_IMAGE (default
#                                 ghcr.io/trycua/macos:26-slim), with this
#                                 branch's cua-spacesd installed in the guest
#
# The Linux image is the published slim image plus this branch's cua-spacesd
# (built by libs/images/linux/build-spacesd-linux.sh), fuse3, /volume and the
# volume helper; it is removed afterwards. The cua home is throwaway, under
# CUA_VOLUME_E2E_HOME (default ${XDG_CACHE_HOME:-~/.cache}/cua/volume-e2e: under
# $HOME so Docker engines that only share the home directory can reach it, and
# short enough for Unix socket paths). Spaces are deleted by the test, pass or
# fail. Set ANTHROPIC_API_KEY to also have a persistent
# agent do a task through the mount.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../../../../../.." && pwd)"
macos=0
[ "${1:-}" = "--macos" ] && macos=1
arch="$(uname -m)"; [ "$arch" = "x86_64" ] && arch=amd64; [ "$arch" = "aarch64" ] && arch=arm64
bin="$repo/libs/images/linux/dist/$arch/cua-spacesd"
[ -x "$bin" ] || "$repo/libs/images/linux/build-spacesd-linux.sh" "$arch"
ctx="$(mktemp -d)"
image="cua-volume-e2e:$$"
cleanup() {
  docker rmi -f "$image" >/dev/null 2>&1 || true
  rm -rf "$ctx"
}
trap cleanup EXIT
cp "$bin" "$ctx/cua-spacesd"
cp "$repo/libs/images/linux/files/supervisor/supervisord.conf" "$ctx/"
cat > "$ctx/Dockerfile" <<'D'
FROM ghcr.io/trycua/linux:24.04-slim
USER root
RUN apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq fuse3 >/dev/null \
 && rm -rf /var/lib/apt/lists/* && install -d -o cua -g cua -m 0755 /volume
COPY cua-spacesd /usr/local/bin/cua-spacesd
COPY supervisord.conf /etc/supervisor/supervisord.conf
D
docker build -q -t "$image" "$ctx" >/dev/null
export CUA_VOLUME_E2E=1 CUA_VOLUME_E2E_LINUX_IMAGE="$image"
export CUA_VOLUME_E2E_HOME="${CUA_VOLUME_E2E_HOME:-${XDG_CACHE_HOME:-$HOME/.cache}/cua/volume-e2e}"
if [ "$macos" = 1 ]; then
  export CUA_VOLUME_E2E_MACOS_IMAGE="${CUA_VOLUME_E2E_MACOS_IMAGE:-ghcr.io/trycua/macos:26-slim}"
  (cd "$repo/libs/cua-spacesd" && cargo build -q --release -p cua-spacesd)
  export CUA_VOLUME_E2E_MACOS_SPACESD="$repo/libs/cua-spacesd/target/release/cua-spacesd"
fi
cd "$repo/libs/cua"
cargo test -p cua-spaces-ext --test volume_e2e -- --nocapture
