#!/usr/bin/env bash
# Retry-once for a single E2E case inside the release gate.
#
#   cua_e2e_run_case NAME LOG_FILE WORK_DIR CMD [ARGS...]
#
# Runs CMD in WORK_DIR, teeing output to LOG_FILE, and returns its exit code.
# When CUA_E2E_RETRY_ONCE=1 a failed case is rerun exactly once:
#   - pass on the rerun: the case is "flaky". It is appended to
#     ${CUA_E2E_FLAKY_FILE} (default: <results dir>/flaky-cases.jsonl) and the
#     function returns 0, so the lane does not fail.
#   - fail on the rerun: two failures are a real failure; returns the rerun's
#     exit code.
# The first attempt's typed result rows, declarations and recordings are moved
# to <results dir>/retried-attempts/NAME/ so the report validator only sees the
# attempt that counts and no orphan video is left behind.
#
# Environment (set by the harness scripts): CUA_E2E_RESULTS_FILE,
# CUA_E2E_DECLARATIONS_FILE, CUA_E2E_RECORDINGS_ROOT.

cua_e2e_run_case() {
  local name="$1" log="$2" workdir="$3"
  shift 3
  local results="${CUA_E2E_RESULTS_FILE:-}"
  local decls="${CUA_E2E_DECLARATIONS_FILE:-}"
  local rec_root="${CUA_E2E_RECORDINGS_ROOT:-}"
  local retry="${CUA_E2E_RETRY_ONCE:-0}"
  local base_dir="" results_lines=0 decl_lines=0 before_dirs=""

  if [[ "${retry}" == 1 && -n "${results}" ]]; then
    base_dir="$(dirname "${results}")"
    results_lines=$(wc -l < "${results}" 2>/dev/null | tr -d ' ' || echo 0)
    [[ -n "${decls}" ]] && decl_lines=$(wc -l < "${decls}" 2>/dev/null | tr -d ' ' || echo 0)
    [[ -n "${rec_root}" && -d "${rec_root}" ]] \
      && before_dirs="$(find "${rec_root}" -mindepth 1 -maxdepth 1 | sort)"
  fi

  local code
  set +e
  (cd "${workdir}" && "$@") 2>&1 | tee "${log}"
  code=${PIPESTATUS[0]}
  set -e
  if [[ "${code}" == 0 || "${retry}" != 1 || -z "${base_dir}" ]]; then
    return "${code}"
  fi

  echo "[RETRY] ${name} failed (exit ${code}); rerunning once"
  local quarantine="${base_dir}/retried-attempts/${name//[^a-zA-Z0-9._-]/_}"
  mkdir -p "${quarantine}/recordings"
  cp "${log}" "${quarantine}/attempt1.log"
  tail -n +"$((results_lines + 1))" "${results}" > "${quarantine}/results.jsonl" || true
  awk -v n="${results_lines}" 'NR <= n' "${results}" > "${results}.keep" && mv "${results}.keep" "${results}"
  if [[ -n "${decls}" ]]; then
    tail -n +"$((decl_lines + 1))" "${decls}" > "${quarantine}/cases.jsonl" || true
    awk -v n="${decl_lines}" 'NR <= n' "${decls}" > "${decls}.keep" && mv "${decls}.keep" "${decls}"
  fi
  if [[ -n "${rec_root}" && -d "${rec_root}" ]]; then
    local d
    while IFS= read -r d; do
      [[ -n "${d}" ]] || continue
      grep -Fxq -- "${d}" <<< "${before_dirs}" || mv "${d}" "${quarantine}/recordings/"
    done < <(find "${rec_root}" -mindepth 1 -maxdepth 1 | sort)
  fi

  local code2
  set +e
  (cd "${workdir}" && "$@") 2>&1 | tee "${log}"
  code2=${PIPESTATUS[0]}
  set -e
  if [[ "${code2}" == 0 ]]; then
    local flaky_file="${CUA_E2E_FLAKY_FILE:-${base_dir}/flaky-cases.jsonl}"
    echo "[FLAKY] ${name} failed once (exit ${code}) then passed; not failing the gate"
    jq -cn --arg name "${name}" --argjson first_exit "${code}" \
      --arg platform "${CUA_E2E_PLATFORM:-$(uname -s)}" \
      '{case: $name, platform: $platform, first_attempt_exit: $first_exit, second_attempt: "passed"}' \
      >> "${flaky_file}"
    return 0
  fi
  echo "[FAIL] ${name} failed twice (exit ${code}, then ${code2}); real failure"
  return "${code2}"
}
