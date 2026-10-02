#!/usr/bin/env bash
# Forces a full Swift rebuild of a package that depends on `CuaSDK` when the
# generated binding changed since that package's last build.
#
#   libs/cua/swift/scripts/fresh-abi.sh <package-dir> [debug|release] [swift build args...]
#
# Extra arguments (for example `--arch x86_64`) select the build directory
# the same way they do for `swift build`.
#
# Why: `CuaSDK` is regenerated whenever the cua SDK grows, and its classes
# and records are not resilient, so a client module bakes in their vtable
# slots and field offsets. SwiftPM does not recompile a client module when
# only an imported module changed (the client's own sources did not), so a
# stale client calls the wrong method: once `Space.streamSession` dispatched
# to `Space.stopHotspot` and `LiveStreamSession.open` crashed on the
# `[String]` it got back. This keeps a stamp of the binding (the Swift
# source, the C header and the library) in the build directory and, when it
# differs, deletes the compiled Swift objects and modules so everything
# recompiles.
set -euo pipefail
pkg="$(cd "${1:?package dir}" && pwd)"
config="${2:-debug}"
shift $(( $# < 2 ? $# : 2 ))
swift_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bin="$(swift build --package-path "$pkg" -c "$config" "$@" --show-bin-path 2>/dev/null)"
stamp="$bin/.cua-sdk-abi"
# The Swift ABI is the binding and its C header; the library behind them
# does not change what clients compile against.
inputs=("$swift_dir/Sources/CuaSDK/CuaSDK.swift" "$swift_dir/Sources/cua_sdkFFI/include/cua_sdkFFI.h")
# Other generated bindings the package compiles against (space-separated
# paths), for example a binding of a library that extends this SDK.
for extra in ${CUA_SWIFT_ABI_EXTRA:-}; do inputs+=("$extra"); done
want="$(shasum -a 256 "${inputs[@]}" | shasum -a 256 | cut -d' ' -f1)"
have="$(cat "$stamp" 2>/dev/null || true)"
if [ "$want" != "$have" ]; then
  if [ -d "$bin" ]; then
    echo "cua SDK binding changed: rebuilding every Swift module in $bin" >&2
    # Only compiler outputs: SwiftPM's own files (output file maps, source
    # lists) must stay, or it fails to plan the build.
    find "$bin" -path "$bin/*.build/*" \( -name '*.o' -o -name '*.swiftdeps' -o -name '*.priors' \) -delete
    find "$bin" \( -path "$bin/Modules/*" -o -path "$bin/*.swiftmodule" \) -name '*.swiftmodule' -prune -exec rm -rf {} +
  fi
  mkdir -p "$bin"
  printf '%s\n' "$want" > "$stamp"
fi
