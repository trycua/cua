#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Prints the names of a Cua Spaces channel as key=value lines (channel,
# app_name, bundle_id, helper_label, url_scheme, home, env), from `CuaChannel`
# (libs/cua/crates/cua-home/src/channel.rs), the one place that owns them
# (CUA-1776). Build and check scripts read these instead of hard-coding a
# name, so the bundle and the Rust code cannot drift apart.
#
#   scripts/channel-names.sh stable|local
#   eval-free use:  names="$(scripts/channel-names.sh local)"
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
channel="${1:?usage: channel-names.sh <stable|local>}"
# From libs/cua, so its rust-toolchain.toml applies.
cd "$here/../../libs/cua"
exec cargo run --quiet --locked -p cua-home --example channel_names -- "$channel"
