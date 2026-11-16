#!/usr/bin/env bash
# Copy the contract (libs/cua/proto: the .proto files and buf.yaml) into
# crates/cua-proto/proto so `cargo package` / `cargo publish` ship it inside
# the cua-proto crate. The copy is git-ignored; in the repository the build
# always reads libs/cua/proto, so a stale copy is never compiled here.
#
# Usage: libs/cua/scripts/vendor-crate-protos.sh
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
src="$here/../proto"
dst="$here/../crates/cua-proto/proto"

rm -rf "$dst"
mkdir -p "$dst"
cp "$src/buf.yaml" "$dst/"
(cd "$src" && find cua -name '*.proto' -print0 | xargs -0 -I{} sh -c 'mkdir -p "$1/$(dirname "$2")" && cp "$2" "$1/$2"' _ "$dst" {})
echo "vendored $(find "$dst" -name '*.proto' | wc -l | tr -d ' ') .proto files into ${dst#"$here/../"}"
