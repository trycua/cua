#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Local dry run of the Linux release job of
# .github/workflows/cd-cua-spacesd.yml: builds cua-spacesd in debian:11
# (native arch only), runs the glibc/OpenSSL gate and packages the assets
# into packaging/release/dist/.
#
#   packaging/release/build-linux-docker.sh
#
# Env overrides:
#   CUA_SPACESD_TARGET_VOLUME  cargo target volume (default
#                                 cua-e2e-spacesd-release-target); give each
#                                 worktree its own
#   PROTOC_VERSION                default 29.3
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../../../.." && pwd)"
case "$(uname -m)" in
  arm64|aarch64) triple=aarch64-unknown-linux-gnu protoc_arch=aarch_64 ;;
  *) triple=x86_64-unknown-linux-gnu protoc_arch=x86_64 ;;
esac
target_volume="${CUA_SPACESD_TARGET_VOLUME:-${CUA_GUESTD_TARGET_VOLUME:-${CUA_ENV_DRIVER_TARGET_VOLUME:-cua-e2e-spacesd-release-target}}}"
mkdir -p "$here/dist"

docker run --rm --name "cua-e2e-spacesd-release-$$" \
  --memory=4g --memory-swap=4g \
  -v "$repo:/src:ro" \
  -v "$target_volume:/target" \
  -v "cua-e2e-spacesd-release-cargo:/cargo" \
  -v "cua-e2e-spacesd-release-rustup:/rustup" \
  -v "$here/dist:/out" \
  -e CARGO_HOME=/cargo -e RUSTUP_HOME=/rustup -e CARGO_TARGET_DIR=/target \
  -e CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" -e CARGO_PROFILE_RELEASE_DEBUG=0 \
  -e CARGO_INCREMENTAL=0 -e CARGO_TERM_COLOR=never \
  -e TRIPLE="$triple" -e PROTOC_ARCH="$protoc_arch" -e PROTOC_VERSION="${PROTOC_VERSION:-29.3}" \
  debian:11 bash -euo pipefail -c '
    /src/libs/cua-spacesd/packaging/release/debian-build-deps.sh >/dev/null
    curl -fsSL -o /tmp/protoc.zip \
      "https://github.com/protocolbuffers/protobuf/releases/download/v$PROTOC_VERSION/protoc-$PROTOC_VERSION-linux-$PROTOC_ARCH.zip"
    unzip -q -o /tmp/protoc.zip -d /usr/local bin/protoc "include/*"
    [ -x /cargo/bin/rustup ] || curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path
    export PATH="/cargo/bin:$PATH"
    rustup toolchain install stable --profile minimal >/dev/null
    cd /src/libs/cua-spacesd
    timeout 3600 cargo build --locked --release --target "$TRIPLE" -p cua-spacesd --bin cua-spacesd
    bin="/target/$TRIPLE/release/cua-spacesd"
    packaging/release/check-linux-binary.sh "$bin" 2.31
    rm -rf /out/*
    packaging/release/package.sh --binary "$bin" --target "$TRIPLE" --out /out
  '
ls -l "$here/dist"
