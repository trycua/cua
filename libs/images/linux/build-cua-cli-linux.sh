#!/usr/bin/env bash
# Build the `cua` CLI for Linux inside Docker and stage it where the linux image build
# expects it: libs/images/linux/dist/<arch>/cua (the slim tier bakes it into /usr/local/bin).
#
#   build-cua-cli-linux.sh [arm64|amd64 ...]     (default: host arch)
#
# CUA_CLI_BIN=/path/to/cua  stages that binary instead of building (CI builds the CLI once per
# arch and hands the same bytes to every job; it must be a linux build for the arch asked for).
#
# Builds in rust:1-bookworm (glibc 2.36, so the binary runs on Ubuntu 24.04's 2.39). The repo
# root is mounted read-only; cargo's registry and target dir live in named Docker volumes, not
# the macOS mount. Native arch builds directly; a foreign arch runs the rust image for that
# platform under binfmt (Rosetta or qemu), which is slow.
#
# Env overrides:
#   RUST_IMAGE                 default rust:1-bookworm
#   CUA_CLI_TARGET_VOLUME      cargo target volume prefix (default cua-cli-linux-target); give
#                              each worktree its own, since concurrent builds of different
#                              sources in one target dir mix their artifacts
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$HERE/../../.." && pwd)"
RUST_IMAGE="${RUST_IMAGE:-rust:1-bookworm}"
VOL="${CUA_CLI_TARGET_VOLUME:-cua-cli-linux-target}"
host_arch() { case "$(uname -m)" in arm64|aarch64) echo arm64 ;; *) echo amd64 ;; esac; }
[ $# -gt 0 ] || set -- "$(host_arch)"

for arch in "$@"; do
    case "$arch" in arm64|amd64) ;; *) echo "unknown arch $arch" >&2; exit 2 ;; esac
    out="$HERE/dist/$arch"
    mkdir -p "$out"
    if [ -n "${CUA_CLI_BIN:-}" ]; then
        install -m 0755 "$CUA_CLI_BIN" "$out/cua"
        ls -l "$out/cua"
        continue
    fi
    echo "==> cua for linux/$arch in $RUST_IMAGE"
    docker run --rm --platform "linux/$arch" \
        -v "$REPO_ROOT:/src:ro" -v "$out:/out" \
        -v "$VOL-$arch:/target" -v "cua-cli-linux-cargo-$arch:/usr/local/cargo/registry" \
        -e CARGO_TARGET_DIR=/target -e CARGO_INCREMENTAL=0 \
        -w /src/libs/cua "$RUST_IMAGE" bash -euc '
            apt-get update -qq
            apt-get install -y -qq --no-install-recommends clang pkg-config cmake nasm protobuf-compiler libprotobuf-dev perl make >/dev/null
            cargo build --locked --release -p cua-cli --features byoc
            install -m 0755 /target/release/cua /out/cua'
    ls -l "$out/cua"
done
