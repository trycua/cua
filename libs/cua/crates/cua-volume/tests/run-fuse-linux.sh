#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
#
# Runs cua-volume's FUSE test (and clippy of the Linux-only FUSE code) in a
# Linux container (FUSE needs /dev/fuse and
# fusermount3, which a Mac does not have). The container gets 6 GiB of
# memory and 4 CPUs, builds into a named volume, and is always removed.
#
#   tests/run-fuse-linux.sh
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LIBS="$(cd "$HERE/../../../.." && pwd)" # libs (the workspace reaches libs/fleet)
IMAGE="${RUST_IMAGE:-rust:1.97.1-bookworm}"
docker run --rm --memory=6g --memory-swap=6g --cpus=4 \
    --device /dev/fuse --cap-add SYS_ADMIN --security-opt apparmor:unconfined \
    -v "$LIBS":/libs:ro -v cua-volume-fuse-target:/target -v cua-volume-fuse-cargo:/usr/local/cargo/registry \
    -e CARGO_TARGET_DIR=/target -e CARGO_BUILD_JOBS=4 -e CUA_DRIVE_FUSE_TEST=1 \
    "$IMAGE" bash -euc '
        apt-get update -qq >/dev/null && apt-get install -y -qq fuse3 >/dev/null
        cd /libs/cua
        timeout 1800 cargo test --locked -p cua-volume --features fuse --test fuse_linux -- --nocapture
        # Keep the test binary for the gVisor run below.
        bin=$(ls -t /target/debug/deps/fuse_linux-* | grep -v "\.d$" | head -1)
        cp "$bin" /target/fuse_linux_test
        timeout 1800 cargo test --locked -p cua-volume --features fuse --lib
        rustup component add clippy >/dev/null 2>&1 || true
        timeout 1800 cargo clippy --locked -p cua-volume --features fuse --all-targets -- -D warnings
    '

# The guest path as spacesd runs it in a Linux container: gVisor with
# SYS_ADMIN (it stays inside gVisor's sandbox), the mount made by root (the
# `cua-spacesd volume-mount` helper runs under sudo; gVisor refuses an
# unprivileged fusermount3), shared with other users (allow_other).
if docker info --format '{{json .Runtimes}}' | grep -q runsc; then
    docker run --rm --runtime runsc --cap-add SYS_ADMIN --memory=2g \
        -v cua-volume-fuse-target:/target:ro -e CUA_DRIVE_FUSE_TEST=1 ubuntu:24.04 bash -euc '
            apt-get update -qq >/dev/null && apt-get install -y -qq fuse3 >/dev/null
            cd /tmp && /target/fuse_linux_test a_space_mounts_its_view_through_the_host --nocapture
        '
else
    echo "runsc is not registered with docker; skipped the gVisor run"
fi
