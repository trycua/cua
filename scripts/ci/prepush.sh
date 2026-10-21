#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
#
# Local pre-push checks: the cheap CI gates that fail pull requests for
# reasons you can find in seconds on your own machine (headers, lockfiles,
# generated files, formatting, clippy on the crates you changed, Windows
# cfg errors, python and web UI contract checks). Run it before every push; it only looks at files that differ
# from the merge base, so an untouched area costs nothing.
#
#   scripts/ci/prepush.sh              # fast checks, then clippy for changed crates
#   scripts/ci/prepush.sh --fast       # skip clippy and Windows checks (seconds)
#   scripts/ci/prepush.sh --bindings   # also regenerate and compare the UniFFI bindings
#   scripts/ci/prepush.sh --base REF   # compare with REF (default: merge base with origin/main)
#
# Heavy cargo commands honour CUA_BUILD_WRAPPER, for example
#   CUA_BUILD_WRAPPER=~/projects/.cua-work/build-slot.sh scripts/ci/prepush.sh
# A check whose tool is missing is reported as SKIPPED, never as passed.
set -uo pipefail

fast=0 bindings=0 base=""
while [ $# -gt 0 ]; do
  case "$1" in
    --fast) fast=1 ;;
    --bindings) bindings=1 ;;
    --base) base="$2"; shift ;;
    -h|--help) sed -n '4,18p' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done

root="$(git rev-parse --show-toplevel)" || exit 2
cd "$root" || exit 2
if [ -z "$base" ]; then
  for r in origin/main main; do
    git rev-parse -q --verify "$r" >/dev/null && { base="$(git merge-base HEAD "$r")"; break; }
  done
fi
[ -n "$base" ] || { echo "cannot find a base commit; pass --base REF" >&2; exit 2; }
# Committed changes plus anything still uncommitted or untracked.
changed="$( { git diff --name-only "$base"...HEAD; git diff --name-only HEAD; git ls-files --others --exclude-standard; } 2>/dev/null | sort -u )"
[ -n "$changed" ] || { echo "no changes against $base"; exit 0; }

wrap=${CUA_BUILD_WRAPPER:-}
failed=() skipped=() passed=()
touches() { printf '%s\n' "$changed" | grep -Eq "$1"; }
step() { # name, command...
  local name="$1"; shift
  echo; echo "== $name"
  if "$@"; then passed+=("$name"); else failed+=("$name"); fi
}
skip() { echo; echo "== $1: SKIPPED ($2)"; skipped+=("$1 ($2)"); }

# Headers, lockfiles, workflows, generated image constants.
if touches '\.(rs|py|ts|tsx|js|mjs|swift|sh)$|^scripts/spdx-headers\.py$'; then
  step "SPDX headers (FSL)" python3 scripts/spdx-headers.py --fsl --check
fi
if touches '(^|/)(Cargo\.(toml|lock)|package(-lock)?\.json|pnpm-lock\.yaml|uv\.lock|Package\.resolved)$'; then
  if command -v npm >/dev/null; then step "lockfiles match manifests" scripts/ci/check-lockfiles.sh
  else skip "lockfiles match manifests" "npm not installed"; fi
fi
if touches '^\.github/(workflows|actions)/'; then
  if command -v actionlint >/dev/null; then step "actionlint" actionlint -shellcheck= -pyflakes=
  else skip "actionlint" "install with: brew install actionlint"; fi
fi
if touches '^(scripts/images/|libs/images/)'; then
  step "image refs and constants" bash -c 'python3 -m unittest discover -s scripts/images/tests && python3 scripts/images/gen-image-constants.py --check && python3 scripts/images/record-image-sizes.py --check'
fi
if touches '^docs/'; then
  if [ -x docs/node_modules/tsx/dist/cli.mjs ] || [ -e docs/node_modules/tsx/dist/cli.mjs ]; then
    step "generated docs are current" bash -c 'for l in cua-driver; do node docs/node_modules/tsx/dist/cli.mjs scripts/docs-generators/runner.ts --library "$l" --check || exit 1; done'
  else skip "generated docs are current" "run: pnpm --dir docs install --frozen-lockfile"; fi
fi

# Rust: formatting, then clippy (and Windows cfg) for the crates that changed.
workspaces=(libs/cua libs/cua-spacesd libs/cua-driver/rust)
crate_of() { # file -> "workspace<TAB>package"
  local d; d="$(dirname "$1")"
  while [ "$d" != "." ] && [ "$d" != "/" ]; do
    if [ -f "$d/Cargo.toml" ] && grep -q '^\[package\]' "$d/Cargo.toml"; then
      printf '%s\t%s\n' "$d" "$(awk '/^\[package\]/{p=1;next} /^\[/{p=0} p && /^name *=/{gsub(/^name *= *"|"[ ]*$/,""); print; exit}' "$d/Cargo.toml")"
      return
    fi
    d="$(dirname "$d")"
  done
}
for ws in "${workspaces[@]}"; do
  files="$(printf '%s\n' "$changed" | grep -E "^$ws/.*\.rs$|^$ws/.*Cargo\.toml$" || true)"
  [ -n "$files" ] || continue
  pkgs=()
  while IFS= read -r f; do
    [ -e "$f" ] || continue
    line="$(crate_of "$f")"; [ -n "$line" ] || continue
    dir="${line%%$'\t'*}"; pkg="${line##*$'\t'}"
    # only members of this workspace (skip nested standalone workspaces)
    [ -n "$pkg" ] && (cd "$ws" && cargo metadata --no-deps --format-version 1 2>/dev/null | grep -q "\"name\":\"$pkg\"") && pkgs+=("$pkg")
  done <<<"$files"
  [ ${#pkgs[@]} -gt 0 ] || continue
  uniq_pkgs="$(printf '%s\n' "${pkgs[@]}" | sort -u)"
  pkgs=(); pflags=()
  for p in $uniq_pkgs; do pkgs+=("$p"); pflags+=(-p "$p"); done
  step "cargo fmt ($ws: ${pkgs[*]})" bash -c "cd $ws && cargo fmt ${pflags[*]} -- --check"
  if [ $fast -eq 0 ]; then
    step "clippy ($ws: ${pkgs[*]})" bash -c "cd $ws && $wrap cargo clippy --locked ${pflags[*]} --all-targets -- -D warnings"
    if cargo xwin --version >/dev/null 2>&1; then
      step "Windows clippy ($ws)" bash -c "cd $ws && $wrap cargo xwin clippy --locked --target x86_64-pc-windows-msvc ${pflags[*]} --all-targets -- -D warnings"
    else
      skip "Windows clippy ($ws)" "install cargo-xwin: cargo install cargo-xwin && rustup target add x86_64-pc-windows-msvc"
    fi
  fi
done

# Python: the same three tools as CI: Lint Python (black, not ruff format).
if touches '^(libs/python/|tests/|pyproject\.toml$)'; then
  if command -v uv >/dev/null; then
    step "python isort, black, ruff" bash -c 'uv run isort --check-only libs/python tests && uv run black --check libs/python tests && uv run ruff check libs/python tests'
  else skip "python isort, black, ruff" "install uv"; fi
fi

# Cua Spaces web UI, bridge contract (Electron/browser binding drift) and Electron shell.
if touches '^(apps/cua-spaces-web/|apps/cua-spaces-desktop/|apps/cua-spaces/core-wasm/|libs/cua/crates/cua-spaces-app-core/)'; then
  if [ -d apps/cua-spaces-web/node_modules ] && [ -d apps/cua-spaces-desktop/node_modules ]; then
    step "web UI typecheck, tests and bridge contract" bash -c 'cd apps/cua-spaces-web && pnpm check'
    step "Electron shell typecheck and router contract" bash -c 'cd apps/cua-spaces-desktop && pnpm test'
  else skip "web UI and Electron contract" "run pnpm install in apps/cua-spaces-web (--ignore-workspace) and apps/cua-spaces-desktop"; fi
fi

if [ $bindings -eq 1 ]; then
  step "UniFFI bindings are current" bash -c "$wrap node libs/cua/scripts/generate-uniffi-bindings.mjs --check"
elif touches '^libs/cua/(crates/cua-sdk|crates/cua-spaces|uniffi|python|typescript|swift|kotlin)'; then
  echo; echo "note: you changed SDK or binding sources; run with --bindings to compare the generated files"
fi

echo; echo "---- summary"
for x in ${passed[@]+"${passed[@]}"}; do echo "pass     $x"; done
for x in ${skipped[@]+"${skipped[@]}"}; do echo "SKIPPED  $x"; done
for x in ${failed[@]+"${failed[@]}"}; do echo "FAIL     $x"; done
[ ${#failed[@]} -eq 0 ]
