#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
swift_dir="$repo_root/cyclops-cs/sdk-bindings/swift"
library="$("$repo_root"/cyclops-cs/scripts/build-sdk-bindings-native.sh)"
library_dir="$(dirname "$library")"
output="$(mktemp "${TMPDIR:-/tmp}/cyclops-swift-sdk.XXXXXX")"
trap 'rm -f "$output"' EXIT
target="$1"
case "$target" in /*) ;; *) target="$repo_root/$target" ;; esac
shift
for header in CyclopsSdkFFI.h CyclopsSdkSchemaFFI.h; do
  [ -f "$swift_dir/$header" ] || { echo "Swift FFI header is missing: $swift_dir/$header" >&2; exit 1; }
done

# Module-map headers resolve relative to their module-map directory.
swiftc_args=(
  -module-name CyclopsSdk
  -I "$swift_dir"
  -Xcc "-fmodule-map-file=$swift_dir/CyclopsSdkFFI.modulemap"
  -Xcc "-fmodule-map-file=$swift_dir/CyclopsSdkSchemaFFI.modulemap"
  -L "$library_dir"
  -lcyclops_sdk
  -Xlinker -rpath
  -Xlinker "$library_dir"
  "$swift_dir/CyclopsSdk.swift"
  "$swift_dir/CyclopsSdkSchema.swift"
  "$target"
  "$@"
  -o "$output"
)
swiftc "${swiftc_args[@]}"
"$output"
