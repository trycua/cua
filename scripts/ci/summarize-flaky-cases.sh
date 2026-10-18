#!/usr/bin/env bash
# Print a markdown "Flaky cases" section for every flaky-cases.jsonl under DIR.
# A flaky case failed once and passed on the single retry (see
# scripts/ci/lib/e2e-retry-once.sh). Prints nothing when there are none, so the
# caller can append the output to a job summary or an issue body unconditionally.
# Usage: summarize-flaky-cases.sh DIR
set -euo pipefail
dir="${1:?usage: summarize-flaky-cases.sh DIR}"
rows=""
while IFS= read -r -d '' file; do
  rows+="$(jq -r '"| \(.platform) | `\(.case)` | \(.first_attempt_exit) | passed |"' "${file}")"$'\n'
done < <(find "${dir}" -type f -name flaky-cases.jsonl -print0 | sort -z)
rows="$(printf '%s' "${rows}" | sed '/^$/d')"
[[ -n "${rows}" ]] || exit 0
printf '## Flaky cases (failed once, passed on retry)\n\n'
printf 'These did not fail the gate. Two failures of the same case are a real failure.\n\n'
printf '| Platform | Case | First exit | Retry |\n| --- | --- | ---: | --- |\n%s\n' "${rows}"
