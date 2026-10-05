#!/usr/bin/env bash
# Check that every lockfile in the repository matches its manifests, without
# building anything. Lockfiles are discovered, so a new workspace is covered as
# soon as its lockfile is added.
#
#   Cargo.lock         cargo metadata --locked (resolves, never writes)
#   pnpm-lock.yaml     pnpm install --frozen-lockfile --lockfile-only, and the
#                      file must come back byte-identical (pnpm version from
#                      the package.json "packageManager" field)
#   package-lock.json  npm install --package-lock-only, compared on the
#                      dependency graph (versions, sources, integrity, ranges)
#   uv.lock            uv lock --check
#
# Every lockfile is restored afterwards: the check never leaves changes.
# libs/fleet is a read-only mirror and is skipped.
#
# Usage: scripts/ci/check-lockfiles.sh [--list] [--kind cargo|pnpm|npm|uv]...
#
# Drift in a path listed in scripts/ci/lockfile-drift-allowlist.txt is a
# warning; a listed path that is clean is an error (remove it from the list).
#
# REQUIRED names lockfiles that drifted unseen before (standalone workspaces
# outside every component's CI): the check fails if discovery ever stops
# finding one of them.

set -uo pipefail

ROOT="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
cd "$ROOT" || exit 2
ALLOWLIST="scripts/ci/lockfile-drift-allowlist.txt"
REQUIRED=(
  examples/streaming/rust/Cargo.lock
  tests/e2e/cua-sdk/rust/Cargo.lock
)

list_only=0
kinds=()
while [ $# -gt 0 ]; do
  case "$1" in
    --list) list_only=1 ;;
    --kind) shift; kinds+=("${1:?--kind needs a value}") ;;
    --kind=*) kinds+=("${1#--kind=}") ;;
    -h|--help) sed -n '2,24p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done
[ ${#kinds[@]} -eq 0 ] && kinds=(cargo pnpm npm uv)

kind_of() {
  case "$(basename "$1")" in
    Cargo.lock) echo cargo ;;
    pnpm-lock.yaml) echo pnpm ;;
    package-lock.json) echo npm ;;
    uv.lock) echo uv ;;
    bun.lock|bun.lockb|yarn.lock) echo unsupported ;;
  esac
}

wanted() {
  local k
  for k in "${kinds[@]}"; do [ "$k" = "$1" ] && return 0; done
  return 1
}

# Tracked and new (not ignored) lockfiles, outside the mirror and build dirs.
discover() {
  git ls-files --cached --others --exclude-standard -- \
    '*Cargo.lock' '*pnpm-lock.yaml' '*package-lock.json' '*uv.lock' \
    '*bun.lock' '*bun.lockb' '*yarn.lock' |
    grep -Ev '(^|/)(node_modules|target)/|^libs/fleet/' | sort -u
}

allowed() {
  [ -f "$ALLOWLIST" ] && grep -Eq "^[[:space:]]*$(printf '%s' "$1" | sed 's/[.[\*^$/]/\\&/g')[[:space:]]*(#.*)?$" "$ALLOWLIST"
}

fix_command() {
  local dir="$1" kind="$2"
  case "$kind" in
    cargo) echo "(cd $dir && cargo update --workspace)" ;;
    pnpm) echo "(cd $dir && pnpm install --lockfile-only --ignore-scripts)" ;;
    npm) echo "(cd $dir && npm install --package-lock-only --ignore-scripts)" ;;
    uv) echo "uv lock --project $dir" ;;
  esac
}

TMP="$(mktemp -d "${TMPDIR:-/tmp}/check-lockfiles.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT

# Run in the lockfile's directory, then put the lockfile back as it was.
with_restore() {
  local lock="$1"; shift
  cp -p "$lock" "$TMP/original"
  ( cd "$(dirname "$lock")" && "$@" ) >"$TMP/log" 2>&1
  local rc=$?
  cp -p "$lock" "$TMP/after"
  cp -p "$TMP/original" "$lock"
  return $rc
}

pnpm_cmd() {
  local dir="$1" pm want have
  pm="$(node -e 'try{process.stdout.write(require(process.argv[1]).packageManager||"")}catch{}' "$ROOT/$dir/package.json" 2>/dev/null)"
  want=""
  case "$pm" in pnpm@*) want="${pm#pnpm@}"; want="${want%%+*}" ;; esac
  have="$(pnpm --version 2>/dev/null || true)"
  if [ -n "$want" ] && [ "$want" != "$have" ]; then
    echo "npx --yes pnpm@$want"
  elif [ -n "$have" ]; then
    echo "pnpm"
  else
    echo "npx --yes pnpm@9"
  fi
}

# The dependency graph of a package-lock.json, for a comparison that ignores
# npm-version-specific metadata (root "repository", key order, ...).
npm_graph_diff() {
  python3 - "$1" "$2" <<'PY'
import json, sys

FIELDS = ("version", "resolved", "integrity", "link", "dev", "optional", "peer",
          "dependencies", "devDependencies", "optionalDependencies",
          "peerDependencies", "peerDependenciesMeta", "bundleDependencies")

def graph(path):
    with open(path) as fh:
        data = json.load(fh)
    return {key: {f: entry.get(f) for f in FIELDS if f in entry}
            for key, entry in (data.get("packages") or {}).items()}

before, after = graph(sys.argv[1]), graph(sys.argv[2])
changes = []
for key in sorted(set(before) | set(after)):
    if key not in before:
        changes.append(f"+ {key or '<root>'}")
    elif key not in after:
        changes.append(f"- {key or '<root>'}")
    elif before[key] != after[key]:
        fields = sorted(f for f in set(before[key]) | set(after[key])
                        if before[key].get(f) != after[key].get(f))
        changes.append(f"~ {key or '<root>'}: {', '.join(fields)}")
for line in changes[:15]:
    print(f"    {line}")
if len(changes) > 15:
    print(f"    ... {len(changes) - 15} more")
sys.exit(1 if changes else 0)
PY
}

check_one() {
  local lock="$1" kind="$2" dir
  dir="$(dirname "$lock")"
  case "$kind" in
    cargo)
      command -v cargo >/dev/null || { echo "cargo not found" >"$TMP/log"; return 3; }
      ( cd "$dir" && cargo metadata --locked --format-version 1 >/dev/null ) 2>"$TMP/log"
      ;;
    pnpm)
      command -v node >/dev/null || { echo "node not found" >"$TMP/log"; return 3; }
      # shellcheck disable=SC2046
      with_restore "$lock" $(pnpm_cmd "$dir") install --frozen-lockfile --lockfile-only \
        --ignore-scripts --config.confirmModulesPurge=false || return 1
      cmp -s "$TMP/original" "$TMP/after" || {
        echo "pnpm rewrote the lockfile (it is not in pnpm's canonical form)" >>"$TMP/log"
        return 1
      }
      ;;
    npm)
      command -v npm >/dev/null || { echo "npm not found" >"$TMP/log"; return 3; }
      with_restore "$lock" npm install --package-lock-only --ignore-scripts --no-audit --no-fund || return 1
      npm_graph_diff "$TMP/original" "$TMP/after" >>"$TMP/log"
      ;;
    uv)
      command -v uv >/dev/null || { echo "uv not found" >"$TMP/log"; return 3; }
      uv lock --check --project "$dir" >"$TMP/log" 2>&1
      ;;
    *)
      echo "no check for $(basename "$lock") yet; add one to $0" >"$TMP/log"
      return 3
      ;;
  esac
}

failures=0 warnings=0 checked=0
while IFS= read -r lock; do
  [ -n "$lock" ] || continue
  kind="$(kind_of "$lock")"
  [ "$kind" = unsupported ] || wanted "$kind" || continue
  if [ $list_only -eq 1 ]; then
    printf '%-6s %s\n' "$kind" "$lock"
    continue
  fi
  checked=$((checked + 1))
  : >"$TMP/log"
  check_one "$lock" "$kind"
  rc=$?
  dir="$(dirname "$lock")"
  if [ $rc -eq 0 ]; then
    if allowed "$lock"; then
      echo "FAIL  $lock is clean but listed in $ALLOWLIST; remove it"
      failures=$((failures + 1))
    else
      echo "ok    $lock"
    fi
    continue
  fi
  if [ $rc -eq 3 ]; then
    echo "FAIL  $lock: $(cat "$TMP/log")"
    failures=$((failures + 1))
    continue
  fi
  if allowed "$lock"; then
    echo "WARN  $lock drifted (allowlisted); fix: $(fix_command "$dir" "$kind")"
    [ -n "${GITHUB_ACTIONS:-}" ] && echo "::warning file=$lock::lockfile drift (allowlisted): $(fix_command "$dir" "$kind")"
    warnings=$((warnings + 1))
  else
    echo "FAIL  $lock drifted; fix: $(fix_command "$dir" "$kind")"
    [ -n "${GITHUB_ACTIONS:-}" ] && echo "::error file=$lock::lockfile drift: $(fix_command "$dir" "$kind")"
    failures=$((failures + 1))
  fi
  tail -n 20 "$TMP/log" | sed 's/^/      /'
done < <(discover)

found="$(discover)"
for lock in "${REQUIRED[@]}"; do
  wanted "$(kind_of "$lock")" || continue
  if ! grep -Fxq "$lock" <<<"$found"; then
    echo "FAIL  $lock is required but was not discovered (moved? update REQUIRED in $0)"
    failures=$((failures + 1))
  fi
done

[ $list_only -eq 1 ] && exit 0
echo "checked $checked lockfile(s): $failures failed, $warnings allowlisted drift"
[ $failures -eq 0 ]
