#!/usr/bin/env bash
# Run every TLA+ model in models.tsv with TLC and compare each result with its
# expected one. Exit 0 only when every row matches.
#   check-models.sh            all rows
#   check-models.sh session    rows whose config path contains "session"
# Needs Java 11+ on PATH (or JAVA=...). Downloads tla2tools.jar v1.7.4 once
# into $TLA2TOOLS_CACHE (default ~/.cache/cua-driver-tla) and checks its sha256.
set -euo pipefail
SKILL="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODELS="$SKILL/models"
FILTER="${1:-}"
JAVA="${JAVA:-java}"
JAR_VERSION=1.7.4
JAR_SHA256=936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88
CACHE="${TLA2TOOLS_CACHE:-$HOME/.cache/cua-driver-tla}"
JAR="${TLA2TOOLS:-$CACHE/tla2tools-$JAR_VERSION.jar}"
PER_RUN_SECONDS="${TLC_SECONDS:-300}"

sha256() { if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1; }
if [[ ! -f "$JAR" ]]; then
  mkdir -p "$(dirname "$JAR")"
  curl -fsSL -o "$JAR.part" \
    "https://github.com/tlaplus/tlaplus/releases/download/v$JAR_VERSION/tla2tools.jar"
  mv "$JAR.part" "$JAR"
fi
[[ "$(sha256 "$JAR")" == "$JAR_SHA256" ]] || { echo "tla2tools.jar sha256 mismatch: $JAR" >&2; exit 2; }
"$JAVA" -version >/dev/null 2>&1 || { echo "Java 11+ is required (set JAVA=...)" >&2; exit 2; }

OUT="$(mktemp -d "${TMPDIR:-/tmp}/cua-driver-tla.XXXXXX")"
failures=0
selected=0
while IFS=$'\t' read -r spec cfg expect; do
  [[ -z "$spec" || "$spec" == \#* ]] && continue
  [[ -n "$FILTER" && "$cfg" != *"$FILTER"* ]] && continue
  selected=$((selected + 1))
  dir="$MODELS"
  name="$(basename "$cfg" .cfg)"
  log="$OUT/$name.out"
  set +e
  ( cd "$dir" && "$JAVA" -XX:+UseParallelGC -cp "$JAR" tlc2.TLC -workers 1 \
      -metadir "$OUT/$name.states" -config "$name.cfg" "$spec" ) >"$log" 2>&1 &
  pid=$!
  ( sleep "$PER_RUN_SECONDS"; kill "$pid" 2>/dev/null ) >/dev/null 2>&1 &
  watchdog=$!
  wait "$pid"
  kill "$watchdog" 2>/dev/null; wait "$watchdog" 2>/dev/null
  set -e
  states="$(sed -n 's/.* \([0-9][0-9]*\) distinct states found.*/\1/p' "$log" | tail -1)"
  if grep -q '^Model checking completed. No error has been found.' "$log"; then
    got=pass
  elif got="$(sed -n 's/^Error: Invariant \([A-Za-z0-9_]*\) is violated.*/\1/p; s/^Error: Temporal properties were violated.*/temporal/p' "$log" | head -1)"; [[ -n "$got" ]]; then
    # TLC does not name the temporal property that failed; a config that
    # checks one names it on its PROPERTIES line.
    if [[ "$got" == temporal ]]; then
      got="$(sed -n 's/^PROPERTIES* *\([A-Za-z0-9_]*\) *$/\1/p' "$MODELS/$cfg" | head -1)"
      [[ -n "$got" ]] || got="a temporal property"
    fi
    :
  else
    got="error"
  fi
  if [[ "$got" == "$expect" ]]; then
    printf 'ok    %-36s %-16s %s states\n' "$cfg" "$got" "${states:-?}"
  else
    printf 'FAIL  %-36s expected %-8s got %-8s (%s)\n' "$cfg" "$expect" "$got" "$log"
    failures=$((failures + 1))
  fi
done <"$SKILL/models.tsv"
echo "TLC output: $OUT"
[[ "$selected" -gt 0 ]] || { echo "no row in models.tsv matches '$FILTER'" >&2; exit 2; }
[[ "$failures" == 0 ]]
