#!/usr/bin/env bash
set -euo pipefail

if (($# != 2)); then
  echo "usage: link-e2e-evidence.sh <summary.md> <artifact-url>" >&2
  exit 2
fi

summary_path="$1"
artifact_url="$2"

awk -F '|' -v artifact_url="${artifact_url}" '
  BEGIN { OFS = "|" }
  function trim(value) {
    sub(/^[[:space:]]+/, "", value)
    sub(/[[:space:]]+$/, "", value)
    return value
  }
  /^\|/ && NF >= 16 {
    cell = trim($2)
    evidence = trim($15)
    if (cell != "Cell" && cell !~ /^-+$/ && evidence != "" && evidence != "-" && evidence !~ /^\[/) {
      $15 = " [" evidence "](" artifact_url ") "
    }
  }
  { print }
' "${summary_path}"
