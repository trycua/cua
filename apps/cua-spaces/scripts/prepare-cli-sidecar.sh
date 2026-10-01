#!/bin/sh
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Stages the Cua Spaces build of the `cua` CLI (`cua-spaces-cli`: the MIT
# `cua` plus the Keyvault, teleport, the Cua Volume and persistent agents) as
# the Tauri sidecar (src-tauri/binaries/cua-<triple>)
# so bundles built with `--config src-tauri/tauri.sidecar.conf.json` ship it
# next to the app executable. The first-run installer puts it on PATH.
#
#   scripts/prepare-cli-sidecar.sh                       # build it for the host
#   scripts/prepare-cli-sidecar.sh --target TRIPLE       # cross target
#   scripts/prepare-cli-sidecar.sh --target universal-apple-darwin
#   scripts/prepare-cli-sidecar.sh --binary path/to/cua --target TRIPLE
#
# Without --binary the CLI is built from libs/cua
# (`cargo build -p cua-spaces-cli`).
set -eu

here="$(cd "$(dirname "$0")/.." && pwd)"
repo="$(cd "$here/../.." && pwd)"
target=""
binary=""
profile=release
while [ $# -gt 0 ]; do
    case "$1" in
    --target)
        target="$2"
        shift
        ;;
    --binary)
        binary="$2"
        shift
        ;;
    --debug) profile=debug ;;
    -h | --help)
        sed -n '4,17p' "$0"
        exit 0
        ;;
    *)
        echo "unknown option: $1" >&2
        exit 2
        ;;
    esac
    shift
done

host_triple() { rustc -vV | sed -n 's/^host: //p'; }
[ -n "$target" ] || target="$(host_triple)"
ext=""
case "$target" in *windows*) ext=".exe" ;; esac
out_dir="$here/src-tauri/binaries"
out="$out_dir/cua-$target$ext"
mkdir -p "$out_dir"

build_one() { # triple -> prints path
    flag=""
    [ "$profile" = release ] && flag="--release"
    # shellcheck disable=SC2086
    cargo build --locked $flag -p cua-spaces-cli --target "$1" --manifest-path "$repo/libs/cua/Cargo.toml" >&2
    printf '%s\n' "${CARGO_TARGET_DIR:-$repo/libs/cua/target}/$1/$profile/cua-spaces-cli$ext"
}

if [ -n "$binary" ]; then
    cp "$binary" "$out"
elif [ "$target" = universal-apple-darwin ]; then
    arm="$(build_one aarch64-apple-darwin)"
    x64="$(build_one x86_64-apple-darwin)"
    lipo -create "$arm" "$x64" -output "$out"
    # `tauri build --target universal-apple-darwin` compiles each slice on its
    # own, and each slice's build script needs its own sidecar.
    for slice in aarch64-apple-darwin:"$arm" x86_64-apple-darwin:"$x64"; do
        cp "${slice#*:}" "$out_dir/cua-${slice%%:*}"
        chmod 0755 "$out_dir/cua-${slice%%:*}"
    done
else
    cp "$(build_one "$target")" "$out"
fi
chmod 0755 "$out"
echo "staged $out"
