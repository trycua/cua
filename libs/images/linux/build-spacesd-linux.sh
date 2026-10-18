#!/usr/bin/env bash
# Build cua-spacesd for Linux inside Docker and stage it where the image
# build expects it: libs/images/linux/dist/<arch>/cua-spacesd.
#
#   build-spacesd-linux.sh [arm64|amd64 ...]     (default: host arch)
#
# Builds in rust:1-bookworm (glibc 2.36, so the binary runs on Ubuntu 24.04's
# 2.39 and older distros too). The repo root is mounted read-only because
# libs/cua-spacesd path-depends on libs/cua-driver/rust crates; cargo's
# registry and target dir live in named Docker volumes, not the macOS mount.
#
# Native arch builds directly. A foreign arch is built with either
#   CROSS_MODE=cross    (default) Debian multiarch cross toolchain + :<arch> -dev libs
#   CROSS_MODE=emulate  the rust image for that platform under binfmt (Rosetta/qemu)
#
# Env overrides:
#   CUA_SPACESD_PACKAGE  cargo package   (default cua-spacesd)
#   CUA_SPACESD_BIN      binary name     (default cua-spacesd)
#   CUA_SPACESD_FEATURES extra --features
#   RUST_IMAGE              default rust:1-bookworm
#   CUA_SPACESD_GIT_SHA  source revision baked into the binary (default: git
#                       HEAD of this checkout; reported by `build-info` and
#                       the doctor)
#   CUA_SPACESD_TARGET_VOLUME  cargo target volume name prefix (default
#                           cua-e2e-spacesd-target); give each worktree its
#                           own, since concurrent builds of different sources
#                           in one target dir mix their artifacts
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$HERE/../../.." && pwd)"
PKG="${CUA_SPACESD_PACKAGE:-${CUA_GUESTD_PACKAGE:-${CUA_ENV_DRIVER_PACKAGE:-cua-spacesd}}}"
BIN="${CUA_SPACESD_BIN:-${CUA_GUESTD_BIN:-${CUA_ENV_DRIVER_BIN:-cua-spacesd}}}"
FEATURES="${CUA_SPACESD_FEATURES:-${CUA_GUESTD_FEATURES:-${CUA_ENV_DRIVER_FEATURES:-}}}"
RUST_IMAGE="${RUST_IMAGE:-rust:1-bookworm}"
CROSS_MODE="${CROSS_MODE:-cross}"
host_arch() { case "$(uname -m)" in arm64|aarch64) echo arm64 ;; *) echo amd64 ;; esac; }
[ $# -gt 0 ] || set -- "$(host_arch)"

# Build dependencies of cua-spacesd + the cua-driver platform-linux crates
# (same set as .github/workflows/cd-rust-cua-driver.yml / ci-rust-linux.yml),
# plus protoc for tonic codegen and a C++ toolchain for openh264-sys2.
DEV_LIBS="libx11-dev libxi-dev libxtst-dev libxext-dev libxkbcommon-dev libdbus-1-dev libwayland-dev libxcb1-dev"
TOOLS="clang pkg-config cmake nasm protobuf-compiler libprotobuf-dev perl make"

for arch in "$@"; do
    case "$arch" in
        arm64) triple=aarch64-unknown-linux-gnu; gnu=aarch64-linux-gnu ;;
        amd64) triple=x86_64-unknown-linux-gnu; gnu=x86_64-linux-gnu ;;
        *) echo "unknown arch $arch" >&2; exit 2 ;;
    esac
    mode=native
    [ "$arch" = "$(host_arch)" ] || mode="$CROSS_MODE"
    out="$HERE/dist/$arch"
    mkdir -p "$out"
    echo "==> cua-spacesd ($PKG/$BIN) for linux/$arch [$mode] in $RUST_IMAGE"

    platform="linux/$(host_arch)"
    setup="apt-get update -qq && apt-get install -y -qq --no-install-recommends $TOOLS $DEV_LIBS >/dev/null"
    cargo_env=""
    if [ "$mode" = emulate ]; then
        platform="linux/$arch"
    elif [ "$mode" = cross ]; then
        cross_libs=""
        for l in $DEV_LIBS; do cross_libs="$cross_libs $l:$arch"; done
        setup="dpkg --add-architecture $arch && apt-get update -qq && apt-get install -y -qq --no-install-recommends $TOOLS gcc-${gnu//_/-} g++-${gnu//_/-} libc6-dev-$arch-cross $cross_libs >/dev/null && (cd /src/libs/cua-spacesd && rustup target add $triple)"
        upper="$(echo "$triple" | tr 'a-z-' 'A-Z_')"
        cargo_env="export CARGO_TARGET_${upper}_LINKER=$gnu-gcc CC_${triple//-/_}=$gnu-gcc CXX_${triple//-/_}=$gnu-g++ \
            PKG_CONFIG_ALLOW_CROSS=1 PKG_CONFIG_LIBDIR=/usr/lib/$gnu/pkgconfig:/usr/share/pkgconfig \
            BINDGEN_EXTRA_CLANG_ARGS='--sysroot=/usr/$gnu -I/usr/include/$gnu';"
    fi
    target_flag=""; target_dir_suffix="release"
    if [ "$mode" = cross ]; then target_flag="--target $triple"; target_dir_suffix="$triple/release"; fi
    feat_flag=""; [ -n "$FEATURES" ] && feat_flag="--features $FEATURES"

    git_sha="${CUA_SPACESD_GIT_SHA:-${CUA_GUESTD_GIT_SHA:-$(git -C "$REPO_ROOT" rev-parse HEAD 2>/dev/null || true)}}"
    vol_suffix="$(echo "$platform" | tr / -)"
    t0=$SECONDS
    docker run --rm --platform "$platform" --memory=6g --memory-swap=6g \
        -v "$REPO_ROOT:/src:ro" \
        -v "cua-e2e-spacesd-cargo-registry-$vol_suffix:/usr/local/cargo/registry" \
        -v "${CUA_SPACESD_TARGET_VOLUME:-${CUA_GUESTD_TARGET_VOLUME:-${CUA_ENV_DRIVER_TARGET_VOLUME:-cua-e2e-spacesd-target}}}-$vol_suffix-$arch:/target" \
        -v "$out:/out" \
        -e CUA_SPACESD_GIT_SHA="$git_sha" \
        -e CARGO_TARGET_DIR=/target -e CARGO_TERM_COLOR=never -e CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" \
        "$RUST_IMAGE" bash -euo pipefail -c "
            { $setup; } || { echo 'toolchain setup failed' >&2; exit 1; }
            $cargo_env
            cd /src/libs/cua-spacesd
            cargo build --release --locked -p '$PKG' --bin '$BIN' $target_flag $feat_flag
            install -m 0755 /target/$target_dir_suffix/$BIN /out/cua-spacesd
            file /out/cua-spacesd 2>/dev/null || true
        "
    echo "==> dist/$arch/cua-spacesd ($((SECONDS - t0))s)"
done
