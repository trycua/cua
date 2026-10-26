#!/usr/bin/env bash
# The shared cases (libs/cua/tests/compat/e2b/cases/*.json) with the Python and
# TypeScript runners against whatever the environment points at (real E2B
# with only E2B_API_KEY set, or the gateway after `eval "$(cua compat env
# --provider e2b)"`), compared with the committed recordings.
#
#   libs/cua/tests/compat/run_cases.sh [--record] [--rules rules-space.json] [--tag t] [--only a,b]
#
# SDK versions: $PY_SDKS (default: PINNED and latest) and $TS_SDKS (same).
# --record writes the recordings (fixtures/cases-{py,ts}-<version>.json); only
# do this against real E2B.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HERE="$ROOT/e2b"
WORK="${CASES_WORK:-$(mktemp -d "${TMPDIR:-/tmp}/cua-e2b-cases.XXXXXX")}"
mkdir -p "$WORK"
RECORD=0; RULES="$HERE/rules-space.json"; EXTRA=()
while [ $# -gt 0 ]; do
  case "$1" in
    --record) RECORD=1 ;;
    --rules) RULES="$2"; shift ;;
    --tag|--only|--skip) EXTRA+=("$1" "$2"); shift ;;
    *) echo "unknown option $1"; exit 2 ;;
  esac; shift
done
PINNED="$(cat "$HERE/PINNED")"
read -r -a PY <<<"${PY_SDKS:-$PINNED latest}"
read -r -a TS <<<"${TS_SDKS:-$PINNED latest}"
status=0
for v in "${PY[@]}"; do
  venv="${CUA_COMPAT_VENVS:-$WORK}/py-$v"
  spec="e2b==$v"; [ "$v" = latest ] && spec="e2b"
  [ -x "$venv/bin/python" ] || python3 -m venv "$venv"
  "$venv/bin/pip" install -q --upgrade "$spec"
  ver="$("$venv/bin/python" -c 'import importlib.metadata as m; print(m.version("e2b"))')"
  run="$WORK/cases-py-$ver.json"
  echo "== python e2b $ver"
  "$venv/bin/python" "$HERE/runner.py" ${EXTRA[@]+"${EXTRA[@]}"} >"$run" 2>"$run.err" || { tail -20 "$run.err"; status=1; continue; }
  if [ "$RECORD" = 1 ]; then cp "$run" "$HERE/fixtures/cases-py-$ver.json"; continue; fi
  python3 "$HERE/compare.py" "$HERE/fixtures/cases-py-$ver.json" "$run" --rules "$RULES" ${EXTRA[@]+--subset} || status=1
done
for v in "${TS[@]}"; do
  dir="${CUA_COMPAT_VENVS:-$WORK}/ts-$v"
  mkdir -p "$dir"
  [ -d "$dir/node_modules/e2b" ] || (cd "$dir" && npm init -y >/dev/null && npm install --ignore-scripts --silent "e2b@$([ "$v" = latest ] && echo latest || echo "$v")")
  ver="$(node -p "require('$dir/node_modules/e2b/package.json').version")"
  run="$WORK/cases-ts-$ver.json"
  echo "== typescript e2b $ver"
  node "$HERE/runner.mjs" --sdk "$dir/node_modules/e2b" ${EXTRA[@]+"${EXTRA[@]}"} >"$run" 2>"$run.err" || { tail -20 "$run.err"; status=1; continue; }
  if [ "$RECORD" = 1 ]; then cp "$run" "$HERE/fixtures/cases-ts-$ver.json"; continue; fi
  python3 "$HERE/compare.py" "$HERE/fixtures/cases-ts-$ver.json" "$run" --rules "$RULES" ${EXTRA[@]+--subset} || status=1
done
exit $status
