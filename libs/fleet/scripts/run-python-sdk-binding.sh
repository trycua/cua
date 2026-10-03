#!/usr/bin/env bash
set -euo pipefail

workspace_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
repo_root="$(cd "$workspace_dir/.." && pwd)"
bindings="$workspace_dir/sdk-bindings/python"
library="$("$workspace_dir"/scripts/build-sdk-bindings-native.sh)"
runtime="$(mktemp -d "${TMPDIR:-/tmp}/cyclops-python-sdk.XXXXXX")"
trap 'rm -rf "$runtime"' EXIT
cp -R "$bindings/fleet_sdk" "$runtime/fleet_sdk"
cp "$library" "$runtime/fleet_sdk/$(basename "$library")"
target="$1"
case "$target" in /*) ;; *) target="$repo_root/$target" ;; esac
shift
PYTHONPATH="$runtime:$bindings${PYTHONPATH:+:$PYTHONPATH}" python3 "$target" "$@"
