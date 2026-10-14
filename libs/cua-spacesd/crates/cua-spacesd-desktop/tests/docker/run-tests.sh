#!/bin/sh
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Run the Linux desktop conformance tests inside the test container.
#
# Host usage (from the repository root; memory-capped, never touches the host desktop):
#   docker run --rm --memory=4g --memory-swap=4g \
#     -v "$PWD":/repo -v envdesktop-target:/target -v envdesktop-cargo:/usr/local/cargo/registry \
#     -e CARGO_TARGET_DIR=/target -w /repo/libs/cua-spacesd cua-envdesktop-test \
#     sh crates/cua-spacesd-desktop/tests/docker/run-tests.sh [cargo test filter]
#
# Starts Xvfb (:99, 1280x800x24) with openbox, a D-Bus session with the AT-SPI
# bus, and PulseAudio with a null sink as the default output, then runs
# `tests/linux_desktop.rs` single-threaded under a timeout.
set -eu
FILTER="${1:-}"
# Big debug test binaries OOM GNU ld at 4 GiB: line-tables-only debug info,
# and rust-lld on aarch64 (scripts/ci/linux/cc-rust-lld.sh; x86_64 already
# links with rust-lld).
export CARGO_PROFILE_DEV_DEBUG="${CARGO_PROFILE_DEV_DEBUG:-line-tables-only}"
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER="${CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER:-$(cd "$(dirname "$0")/../../../../../.." && pwd)/scripts/ci/linux/cc-rust-lld.sh}"
export DISPLAY=:99
export CUA_ENV_LINUX_DESKTOP_TESTS=1
export NO_AT_BRIDGE=0
export GTK_MODULES=gail:atk-bridge
rm -f /tmp/.X99-lock
Xvfb :99 -screen 0 1280x800x24 -nolisten tcp -ac +extension RANDR >/tmp/xvfb.log 2>&1 &
for _ in $(seq 1 50); do xdpyinfo >/dev/null 2>&1 && break; sleep 0.1; done
cargo build -q -p cua-spacesd-test-apps --bin cua-spacesd-x11-pad
export CUA_ENV_X11_PAD="${CARGO_TARGET_DIR:-target}/debug/cua-spacesd-x11-pad"
export CUA_ENV_GTK_FIXTURE="$(pwd)/crates/cua-spacesd-test-apps/fixtures/linux/gtk_button.py"
exec dbus-run-session -- sh -c '
  set -eu
  /usr/libexec/at-spi-bus-launcher --launch-immediately >/tmp/atspi.log 2>&1 &
  openbox >/tmp/openbox.log 2>&1 &
  pulseaudio --daemonize=yes --exit-idle-time=-1 --log-target=file:/tmp/pulse.log >/dev/null 2>&1 || true
  for _ in $(seq 1 50); do pactl info >/dev/null 2>&1 && break; sleep 0.1; done
  pactl load-module module-null-sink sink_name=cua_desktop >/dev/null 2>&1 || true
  pactl set-default-sink cua_desktop >/dev/null 2>&1 || true
  sleep 0.5
  timeout 1500 cargo test -p cua-spacesd-desktop --test linux_desktop -- --test-threads=1 --nocapture '"$FILTER"'
'
