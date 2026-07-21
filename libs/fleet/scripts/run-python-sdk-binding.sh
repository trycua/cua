#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
bindings="$repo_root/cyclops-cs/sdk-bindings/python"
library="$("$repo_root"/cyclops-cs/scripts/build-sdk-bindings-native.sh)"
runtime="$(mktemp -d "${TMPDIR:-/tmp}/cyclops-python-sdk.XXXXXX")"
trap 'rm -rf "$runtime"' EXIT
cp -R "$bindings/cyclops_sdk" "$runtime/cyclops_sdk"
cp "$library" "$runtime/cyclops_sdk/$(basename "$library")"
target="$1"
case "$target" in /*) ;; *) target="$repo_root/$target" ;; esac
shift
PYTHONPATH="$runtime:$bindings${PYTHONPATH:+:$PYTHONPATH}" python3 "$target" "$@"
