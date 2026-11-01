#!/usr/bin/env bash
# Publish the cua SDK's Rust crates to crates.io: `cua-sdk`,
# `cua-spacesd-client` and every workspace crate they depend on (normal and
# build dependencies), in dependency order. The list is computed from
# `cargo metadata`, so a new dependency of the SDK is published with it.
#
# Usage: libs/cua/scripts/publish-crates.sh [--list | --publish <version>]
#   No arguments: `cargo publish --dry-run` for every crate (packages them,
#   builds each from its .crate against the others; needs no token).
#   --list: print the crates, one per line, and exit.
#   --publish <version>: upload every crate not yet on crates.io at
#   <version> (a re-run skips those already published); the workspace must
#   be at <version>. Needs CARGO_REGISTRY_TOKEN.
#
# Files a crate embeds from elsewhere in the repository (the `FILES` table
# of its build.rs, and cua-proto's .proto files) are copied into the crate
# before packaging and removed again afterwards.
set -euo pipefail

ROOTS=(cua-sdk cua-spacesd-client)

mode="${1:-}"
version="${2:-}"
case "$mode" in
  "" | --list) ;;
  --publish) [[ -n "$version" ]] || { echo "usage: publish-crates.sh [--list | --publish <version>]" >&2; exit 2; } ;;
  *) echo "usage: publish-crates.sh [--list | --publish <version>]" >&2; exit 2 ;;
esac
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
workspace="$(cd "$here/.." && pwd)"
cd "$workspace"

# name<TAB>version<TAB>crate dir, for every crate to publish.
CRATE_ROWS="$(cargo metadata --format-version 1 --locked | python3 -c '
import json, os, sys
roots = sys.argv[1:]
m = json.load(sys.stdin)
pkgs = {p["id"]: p for p in m["packages"]}
members = set(m["workspace_members"])
nodes = {n["id"]: n for n in m["resolve"]["nodes"]}
by_name = {pkgs[i]["name"]: i for i in members}
seen, stack = set(), [by_name[r] for r in roots]
while stack:
    i = stack.pop()
    if i in seen:
        continue
    seen.add(i)
    for d in nodes[i]["deps"]:
        if d["pkg"] in members and any(k["kind"] in (None, "build") for k in d["dep_kinds"]):
            stack.append(d["pkg"])
for i in sorted(seen, key=lambda i: pkgs[i]["name"]):
    p = pkgs[i]
    print(p["name"], p["version"], os.path.dirname(p["manifest_path"]), sep="\t")
' "${ROOTS[@]}")"

CRATES=()
while IFS=$'\t' read -r name crate_version dir; do
  CRATES+=("$name")
  if [[ -n "$version" && "$crate_version" != "$version" ]]; then
    echo "error: $name is $crate_version, not $version" >&2
    exit 1
  fi
done <<< "$CRATE_ROWS"

if [[ "$mode" == "--list" ]]; then
  printf '%s\n' "${CRATES[@]}"
  exit 0
fi

# Vendor what the crates embed from outside their directories.
vendored=()
cleanup() { [[ ${#vendored[@]} -eq 0 ]] || rm -rf "${vendored[@]}"; }
trap cleanup EXIT
"$here/vendor-crate-protos.sh"
while IFS=$'\t' read -r name _ dir; do
  [[ -f "$dir/build.rs" ]] && grep -q '^const FILES: ' "$dir/build.rs" || continue
  mkdir -p "$dir/vendor"
  vendored+=("$dir/vendor")
  # Rows of the table: ("<repository path from the crate>", "<name>").
  python3 -c '
import re, sys
text = open(sys.argv[1]).read()
table = text[text.index("const FILES: "):]
table = table[: table.index("];")]
for source, name in re.findall(r"\(\s*\"([^\"]+)\",\s*\"([^\"]+)\",?\s*\)", table):
    print(source, name, sep="\t")
' "$dir/build.rs" |
    while IFS=$'\t' read -r source file; do
      cp -R "$dir/$source" "$dir/vendor/$file"
    done
done <<< "$CRATE_ROWS"

package_args=()
for crate in "${CRATES[@]}"; do package_args+=(-p "$crate"); done

# --allow-dirty: the vendored copies are uncommitted files, copied from this
# commit just above.
if [[ "$mode" != "--publish" ]]; then
  # The dry run unpacks each .crate into a local registry under
  # $CARGO_HOME/registry/src/-<hash>/ and builds the dependents against it.
  # cargo treats registry sources as immutable: an earlier dry run's unpacked
  # copy, and the build scripts compiled from it, would be reused even though
  # the crate changed since. Drop both first.
  cargo_home="${CARGO_HOME:-$HOME/.cargo}"
  check_target="$workspace/target/publish-check"
  for crate in "${CRATES[@]}"; do
    rm -rf "$cargo_home"/registry/src/-*/"$crate"-[0-9]* \
      "$check_target"/*/build/"$crate"-* "$check_target"/*/.fingerprint/"$crate"-*
  done
  CARGO_TARGET_DIR="$check_target" cargo publish --dry-run --locked --allow-dirty "${package_args[@]}"
  echo "dry run: ${#CRATES[@]} crates packaged and verified, nothing uploaded: ${CRATES[*]}"
  exit 0
fi

: "${CARGO_REGISTRY_TOKEN:?CARGO_REGISTRY_TOKEN is required to publish}"
published() {
  [[ "$(curl -sS -o /dev/null -w '%{http_code}' \
    -A 'trycua/cua release (https://github.com/trycua/cua)' \
    "https://crates.io/api/v1/crates/$1/$2")" == 200 ]]
}
pending=()
for crate in "${CRATES[@]}"; do
  if published "$crate" "$version"; then
    echo "$crate $version is already on crates.io; skipping"
  else
    pending+=(-p "$crate")
  fi
done
if [[ ${#pending[@]} -eq 0 ]]; then
  echo "every crate is already on crates.io at $version"
  exit 0
fi
# One invocation: cargo orders the crates and waits for each to reach the
# index before publishing the ones that depend on it. --no-verify: the
# release's dry run (protos-and-crates in cd-cua-sdk.yml) built every crate
# from its package at this commit already; building them again here would
# only make a partial publish (a timeout halfway) more likely.
cargo publish --locked --allow-dirty --no-verify "${pending[@]}"
