#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Cua AI, Inc.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../../../../.." && pwd)
RUN=$(mktemp -d)
trap 'rm -rf "$RUN"' EXIT
clang -Wall -Wextra -Werror "$ROOT/libs/cua-driver/rust/crates/cua-driver/tests/fixtures/inherited_host.c" -o "$RUN/host"
cp "$RUN/host" "$RUN/substitute"
codesign --force --sign - --options runtime --identifier org.example.embedded-fixture "$RUN/host"
codesign --force --sign - --options runtime --identifier org.example.substitute "$RUN/substitute"
HASH=$(codesign -dvv "$RUN/host" 2>&1 | sed -n 's/^CDHash=//p')
[[ -n "$HASH" ]]
export CUA_DRIVER_EMBEDDED_PARENT_REQUIREMENT="cdhash H\"$HASH\""
export CUA_TEST_SIGNED_HOST="$RUN/host" CUA_TEST_SUBSTITUTE_HOST="$RUN/substitute"
cd "$ROOT/libs/cua-driver/rust"
cargo build -p cua-driver --bin cua-driver
codesign --force --sign - --options runtime --identifier org.example.embedded-driver "${CARGO_TARGET_DIR:-target}/debug/cua-driver"
cargo test -p cua-driver --test inherited_stream_test -- --ignored --nocapture
