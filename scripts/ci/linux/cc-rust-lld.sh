#!/bin/sh
# Linker for aarch64 Linux test containers: `cc` driving the toolchain's
# bundled rust-lld. GNU ld (aarch64 Linux's default) is OOM-killed linking the
# big debug test binaries inside a 4 GiB container; lld does not keep every
# input in memory. x86_64 Linux already links with rust-lld (Rust 1.90+).
#
#   -e CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=/repo/scripts/ci/linux/cc-rust-lld.sh
set -eu
sysroot="$(rustc --print sysroot)"
host="$(rustc -vV | sed -n 's/^host: //p')"
exec "${CC:-cc}" -B"$sysroot/lib/rustlib/$host/bin/gcc-ld" -fuse-ld=lld "$@"
