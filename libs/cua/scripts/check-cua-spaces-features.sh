#!/usr/bin/env bash
# Checks that cua-spaces compiles with reduced feature sets, not just with
# its defaults (in-repo builds turn the defaults on through Cargo feature
# unification, which hides gating mistakes). Run from anywhere:
#
#   libs/cua/scripts/check-cua-spaces-features.sh
#
# Extra arguments go to every `cargo check` (for example `--locked`).
set -uo pipefail

cd "$(dirname "$0")/.."

ALL="spaces-files spaces-stream spaces-presence spaces-hotspot spaces-volume spaces-agents mcp mcp-http mcp-client"

# The default set without the named features.
without() {
	local out=() f skip
	for f in $ALL; do
		skip=
		for s in "$@"; do [ "$f" = "$s" ] && skip=1; done
		[ -z "$skip" ] && out+=("$f")
	done
	local IFS=,
	echo "${out[*]}"
}

SETS=(
	""
	"$(without spaces-agents)"
	"$(without spaces-presence)"
	"$(without mcp mcp-http mcp-client)"
	# The Cua Spaces app's set (apps/cua-spaces/src-tauri/Cargo.toml).
	"spaces-files,spaces-stream,spaces-presence,spaces-hotspot,spaces-agents"
	"spaces-stream,spaces-presence,spaces-agents"
	"spaces-stream,spaces-agents,mcp"
)
for f in $ALL; do SETS+=("$f"); done

failed=()
for set in "${SETS[@]}"; do
	echo "==> cua-spaces --no-default-features --features '${set}'"
	if ! cargo check -q -p cua-spaces --no-default-features --features "$set" "$@"; then
		failed+=("'${set}'")
	fi
done
echo "==> cua-spaces (default features)"
cargo check -q -p cua-spaces "$@" || failed+=("default")

if [ ${#failed[@]} -gt 0 ]; then
	echo "cua-spaces failed to compile with: ${failed[*]}" >&2
	exit 1
fi
echo "cua-spaces compiles with every checked feature set."
