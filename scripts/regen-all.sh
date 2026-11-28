#!/usr/bin/env bash
# One command for every generated artifact CI compares against the repo.
#
#   scripts/regen-all.sh                     regenerate everything
#   scripts/regen-all.sh --check             CI parity: fail if anything is stale, write nothing
#   scripts/regen-all.sh --only swift,electron
#   scripts/regen-all.sh --list
#
# Targets (in run order) and the CI check each one mirrors:
#   ffi-mirror  libs/cua/scripts/generate-app-core-ffi.py            (ci-cua-spaces-macos: ffi)
#   contract    cua-spaces-contract-gen, cua-spaces-control-fixtures  (ci-cua-sdk: bindings)
#   sdk         libs/cua/scripts/generate-uniffi-bindings.mjs         (ci-cua-sdk: bindings;
#               Python, Swift, Kotlin, TypeScript SDK bindings + presence art)
#   swift       libs/spaces-app-swift/scripts/generate-bindings.mjs   (ci-cua-spaces-macos: bindings)
#   electron    apps/cua-spaces-desktop `pnpm native --bindings`      (ci-cua-spaces-macos: bindings)
#   driver      cua-contract-gen, libs/cua-driver UniFFI bindings,    (ci-cua-driver-contract-clients)
#               cua-driver-abi-header
#   images      scripts/images/gen-image-constants.py                 (ci-check-image-refs)
#   docs        docs-generators runner, only the libraries the diff   (ci-check-docs)
#               against origin/main touches (all with DOCS_ALL=1)
#
# Not generated, so not here: bridge-shapes.json is hand-written and checked by
# the bridge contract tests; the fleet mirror guard only blocks edits to libs/fleet;
# record-image-sizes.py needs crane and the network (run it yourself).
#
# swift and electron read one release build of cua-spaces-ffi, built once here
# (cargo no-ops when it is current). Skip it with REGEN_SKIP_BUILD=1.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

ALL=(ffi-mirror contract sdk swift electron driver images docs)
check=0
only=""
while [ $# -gt 0 ]; do
  case "$1" in
    --check) check=1 ;;
    --only) only="${2:-}"; shift ;;
    --only=*) only="${1#--only=}" ;;
    --list) printf '%s\n' "${ALL[@]}"; exit 0 ;;
    -h|--help) sed -n '2,/^set -euo/p' "$0" | sed '$d;s/^# \{0,1\}//'; exit 0 ;;
    *) echo "regen-all: unknown argument $1 (try --help)" >&2; exit 2 ;;
  esac
  shift
done

selected=("${ALL[@]}")
if [ -n "$only" ]; then
  IFS=',' read -r -a selected <<<"$only"
  for t in "${selected[@]}"; do
    case " ${ALL[*]} " in *" $t "*) ;; *) echo "regen-all: unknown target '$t'; one of: ${ALL[*]}" >&2; exit 2 ;; esac
  done
fi
want() { local t; for t in "${selected[@]}"; do [ "$t" = "$1" ] && return 0; done; return 1; }
flag() { [ "$check" = 1 ] && echo "--check" || true; }

step() { printf '\n==> %s\n' "$*"; }
failed=()
attempt() { # attempt <target> <command...>
  local t="$1"; shift
  step "$t"
  if ! "$@"; then failed+=("$t"); fi
}

cua_release_dylib() {
  case "$(uname -s)" in Darwin) echo libcua_spaces_ffi.dylib ;; MINGW*|MSYS*|CYGWIN*) echo cua_spaces_ffi.dll ;; *) echo libcua_spaces_ffi.so ;; esac
}

build_ffi_once() {
  [ "${REGEN_SKIP_BUILD:-0}" = 1 ] && return 0
  step "build cua-spaces-ffi (release, once)"
  (cd libs/cua && cargo build --locked --release -p cua-spaces-ffi)
}

run_ffi_mirror() { (cd libs/cua && python3 scripts/generate-app-core-ffi.py $(flag)); }
run_contract() {
  (cd libs/cua \
    && cargo run --locked -q -p cua-spaces-contract --bin cua-spaces-contract-gen -- $(flag) \
    && cargo run --locked -q -p cua-spaces --bin cua-spaces-control-fixtures -- $(flag))
}
run_sdk() {
  [ -d libs/cua/typescript/node_modules/uniffi-bindgen-react-native ] || npm --prefix libs/cua/typescript ci --ignore-scripts
  node libs/cua/scripts/generate-uniffi-bindings.mjs $(flag)
}
run_swift() { node libs/spaces-app-swift/scripts/generate-bindings.mjs $(flag) --skip-build; }
run_electron() {
  (cd apps/cua-spaces-desktop \
    && { [ -d node_modules ] || pnpm install --frozen-lockfile; } \
    && pnpm native -- --skip-build "$([ "$check" = 1 ] && echo --check-bindings || echo --bindings)")
}
run_driver() {
  [ -d libs/cua-driver/typescript/node_modules/uniffi-bindgen-react-native ] || npm --prefix libs/cua-driver/typescript ci --ignore-scripts
  (cd libs/cua-driver/rust && cargo run --locked -p cua-driver-contract --bin cua-contract-gen -- all $(flag)) \
    && (cd libs/cua-driver && node scripts/generate-uniffi-bindings.mjs $(flag)) \
    && (cd libs/cua-driver/rust && cargo run --locked -p cua-driver-bindgen --bin cua-driver-abi-header -- $(flag))
}
run_images() { python3 scripts/images/gen-image-constants.py $(flag); }
run_docs() {
  local tsx=docs/node_modules/tsx/dist/cli.mjs libs
  if [ ! -f "$tsx" ]; then
    # CI installs docs with the root packageManager's pnpm, not docs/'s own.
    npx --yes "pnpm@$(node -p "require('./package.json').packageManager.split('@')[1].split('+')[0]")" --dir docs install --frozen-lockfile
  fi
  if [ "${DOCS_ALL:-0}" = 1 ]; then
    libs="$(node "$tsx" scripts/docs-generators/runner.ts --list 2>/dev/null | grep -oE '^[[:space:]]*[a-z][a-z0-9-]*' | tr -d ' ' | tr '\n' ' ')"
  else
    local base changed
    base="$(git merge-base HEAD origin/main 2>/dev/null || git rev-parse HEAD)"
    changed="$(mktemp)"
    { git diff --name-only "$base"; git ls-files --others --exclude-standard; } >"$changed"
    libs="$(node "$tsx" scripts/docs-generators/runner.ts --changed-files-file "$changed" | tail -1)"
    rm -f "$changed"
  fi
  if [ -z "${libs// /}" ]; then echo "no generated reference is affected by this change"; return 0; fi
  local lib rc=0
  for lib in $libs; do
    node "$tsx" scripts/docs-generators/runner.ts --library "$lib" $(flag) || rc=1
  done
  return $rc
}

# Docs and the contract run first only where they do not change Rust sources;
# the FFI mirror rewrites Rust, so it runs before the build.
want ffi-mirror && attempt ffi-mirror run_ffi_mirror
if want swift || want electron; then
  step "prepare"
  build_ffi_once || { failed+=("build"); }
fi
want contract && attempt contract run_contract
want sdk && attempt sdk run_sdk
want swift && attempt swift run_swift
want electron && attempt electron run_electron
want driver && attempt driver run_driver
want images && attempt images run_images
want docs && attempt docs run_docs

echo
if [ ${#failed[@]} -gt 0 ]; then
  if [ "$check" = 1 ]; then
    echo "regen-all: STALE or failed: ${failed[*]}" >&2
    echo "Fix: scripts/regen-all.sh --only $(IFS=,; echo "${failed[*]}"), then commit the result." >&2
  else
    echo "regen-all: failed: ${failed[*]}" >&2
  fi
  exit 1
fi
if [ "$check" = 1 ]; then echo "regen-all: every selected artifact is up to date."
else echo "regen-all: done. Review with git status, then commit the generated files."; fi
