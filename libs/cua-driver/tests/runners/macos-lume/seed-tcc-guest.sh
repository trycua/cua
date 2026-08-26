#!/usr/bin/env bash
# Guest-side helper: derive CuaDriver's signed requirement and seed system TCC.
set -euo pipefail

TCC_DB="/Library/Application Support/com.apple.TCC/TCC.db"
APP_PATH="${CUA_TCC_APP_PATH:-/Applications/CuaDriverLocal.app}"
BINARY_PATH="${CUA_TCC_BINARY_PATH:-}"
EXPECTED_CLIENT="${CUA_TCC_EXPECTED_CLIENT:-com.trycua.driver.local}"
ALLOW_ADHOC="${CUA_TCC_ALLOW_ADHOC:-0}"
READ_SUDO_PASSWORD="${CUA_TCC_READ_SUDO_PASSWORD:-0}"

usage() {
  cat <<'USAGE'
Usage: seed-tcc-guest.sh [--app PATH] [--binary PATH]
                         [--expected-client ID] [--allow-adhoc]

Run inside a SIP-disabled Lume macOS VM. Seeds kTCCServiceAccessibility and
kTCCServiceScreenCapture into the system TCC database for the signed CuaDriver
app identity. The script refuses to run outside VirtualMac or with SIP enabled.

Pass --app /path/to/binary without --binary for a path-type test grant; also
pass --expected-client with that binary's canonical path. When --app points to
an app bundle, --binary may only name that bundle's executable.
USAGE
}

while (($#)); do
  case "$1" in
    --app)
      if (($# < 2)); then echo "--app requires a value" >&2; exit 2; fi
      APP_PATH="$2"; shift
      ;;
    --app=*) APP_PATH="${1#*=}" ;;
    --binary)
      if (($# < 2)); then echo "--binary requires a value" >&2; exit 2; fi
      BINARY_PATH="$2"; shift
      ;;
    --binary=*) BINARY_PATH="${1#*=}" ;;
    --expected-client)
      if (($# < 2)); then echo "--expected-client requires a value" >&2; exit 2; fi
      EXPECTED_CLIENT="$2"; shift
      ;;
    --expected-client=*) EXPECTED_CLIENT="${1#*=}" ;;
    --allow-adhoc) ALLOW_ADHOC=1 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

fail() {
  echo "seed-tcc: $*" >&2
  exit 2
}

canonical_path() {
  local path="$1"
  local dir
  local base
  dir="$(/usr/bin/dirname "${path}")"
  base="$(/usr/bin/basename "${path}")"
  (cd "${dir}" && printf '%s/%s\n' "$(/bin/pwd -P)" "${base}")
}

bundle_executable_path() {
  local app="$1"
  local info_plist="${app}/Contents/Info.plist"
  local executable_name
  [[ -d "${app}" ]] || fail "app bundle is missing: ${app}"
  [[ -f "${info_plist}" ]] || fail "missing Info.plist at ${info_plist}"
  executable_name="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "${info_plist}" 2>/dev/null || true)"
  [[ -n "${executable_name}" ]] || fail "CFBundleExecutable is missing from ${info_plist}"
  printf '%s/Contents/MacOS/%s\n' "${app}" "${executable_name}"
}

sql_escape() {
  /usr/bin/sed "s/'/''/g" <<< "$1"
}

run_sudo() {
  if [[ "$(/usr/bin/id -u)" -eq 0 ]]; then
    "$@"
  elif [[ -n "${SUDO_PASSWORD_CACHE:-}" ]]; then
    printf '%s\n' "${SUDO_PASSWORD_CACHE}" | /usr/bin/sudo -S -p '' "$@"
  else
    /usr/bin/sudo -n "$@"
  fi
}

MODEL="$(/usr/sbin/sysctl -n hw.model 2>/dev/null || true)"
[[ "${MODEL}" == VirtualMac* ]] || fail "refusing to edit TCC outside a Lume/VirtualMac guest (hw.model=${MODEL:-unknown})"

SIP_STATUS="$(/usr/bin/csrutil status 2>&1 || true)"
[[ "${SIP_STATUS}" == *"System Integrity Protection status: disabled."* ]] \
  || [[ "${SIP_STATUS}" == *"status: disabled"* ]] \
  || fail "system TCC.db is SIP-protected; expected SIP disabled, got: ${SIP_STATUS}"

[[ -n "${EXPECTED_CLIENT}" ]] || fail "--expected-client must not be empty"

CLIENT_TYPE=1
CLIENT=""
BUNDLE_ID=""
SIGN_TARGET=""
if [[ -n "${APP_PATH}" && -d "${APP_PATH}" ]]; then
  INFO_PLIST="${APP_PATH}/Contents/Info.plist"
  APP_EXECUTABLE_PATH="$(bundle_executable_path "${APP_PATH}")"
  if [[ -n "${BINARY_PATH}" ]]; then
    [[ -x "${BINARY_PATH}" ]] || fail "CuaDriver executable is missing or not executable: ${BINARY_PATH}"
    RESOLVED_BINARY_PATH="$(canonical_path "${BINARY_PATH}")"
    RESOLVED_APP_EXECUTABLE_PATH="$(canonical_path "${APP_EXECUTABLE_PATH}")"
    [[ "${RESOLVED_BINARY_PATH}" == "${RESOLVED_APP_EXECUTABLE_PATH}" ]] \
      || fail "--binary must match ${APP_PATH}'s CFBundleExecutable (${APP_EXECUTABLE_PATH}); got ${BINARY_PATH}"
  else
    BINARY_PATH="${APP_EXECUTABLE_PATH}"
    [[ -x "${BINARY_PATH}" ]] || fail "CuaDriver executable is missing or not executable: ${BINARY_PATH}"
  fi
  BUNDLE_ID="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "${INFO_PLIST}" 2>/dev/null || true)"
  [[ -n "${BUNDLE_ID}" ]] || fail "CFBundleIdentifier is missing from ${INFO_PLIST}"
  CLIENT_TYPE=0
  CLIENT="${BUNDLE_ID}"
  SIGN_TARGET="${APP_PATH}"
else
  [[ "${APP_PATH}" != *.app ]] || fail "app bundle is missing: ${APP_PATH}"
  [[ -z "${BINARY_PATH}" ]] \
    || fail "--binary requires --app to point at an app bundle; omit --binary and pass the executable path as --app for a path-type grant"
  BINARY_PATH="${APP_PATH}"
  [[ -x "${BINARY_PATH}" ]] || fail "CuaDriver executable is missing or not executable: ${BINARY_PATH}"
  CLIENT="$(canonical_path "${BINARY_PATH}")"
  SIGN_TARGET="${CLIENT}"
fi

VERIFY_INFO=""
if ! VERIFY_INFO="$(/usr/bin/codesign --verify --deep --strict "${SIGN_TARGET}" 2>&1)"; then
  fail "codesign verification failed for ${SIGN_TARGET}: ${VERIFY_INFO}"
fi

CODESIGN_INFO=""
if ! CODESIGN_INFO="$(/usr/bin/codesign -dvvv "${SIGN_TARGET}" 2>&1)"; then
  fail "could not read codesign metadata from ${SIGN_TARGET}: ${CODESIGN_INFO}"
fi
SIGNING_IDENTIFIER="$(/usr/bin/awk -F= '/^Identifier=/ { print $2; exit }' <<< "${CODESIGN_INFO}")"
[[ -n "${SIGNING_IDENTIFIER}" ]] || fail "could not read signing identifier from ${SIGN_TARGET}"

[[ "${CLIENT}" == "${EXPECTED_CLIENT}" ]] \
  || fail "TCC client mismatch: expected ${EXPECTED_CLIENT}, resolved ${CLIENT}"
if [[ "${CLIENT_TYPE}" == 0 ]]; then
  [[ "${SIGNING_IDENTIFIER}" == "${EXPECTED_CLIENT}" ]] \
    || fail "codesign identifier mismatch: expected ${EXPECTED_CLIENT}, got ${SIGNING_IDENTIFIER}"
fi

REQUIREMENT_INFO=""
if ! REQUIREMENT_INFO="$(/usr/bin/codesign -d -r- "${SIGN_TARGET}" 2>&1)"; then
  fail "could not read designated requirement from ${SIGN_TARGET}: ${REQUIREMENT_INFO}"
fi
REQUIREMENT="$(/usr/bin/awk -F 'designated => ' '/^(# )?designated =>/ { print $2; exit }' <<< "${REQUIREMENT_INFO}")"
[[ -n "${REQUIREMENT}" ]] || fail "could not read designated requirement from ${SIGN_TARGET}"
if [[ "${ALLOW_ADHOC}" != 1 && "${REQUIREMENT}" != *"certificate leaf"* ]]; then
  fail "${SIGN_TARGET} is not signed with a certificate-backed identity; rerun install-local with --require-stable-signing or pass --allow-adhoc for a one-build-only grant"
fi

CSREQ_TMPDIR="$(/usr/bin/mktemp -d /tmp/cua-driver-tcc-csreq.XXXXXX)"
cleanup() { /bin/rm -rf "${CSREQ_TMPDIR}"; }
trap cleanup EXIT
printf '%s\n' "${REQUIREMENT}" > "${CSREQ_TMPDIR}/requirement.txt"
if ! /usr/bin/csreq -r "${CSREQ_TMPDIR}/requirement.txt" -b "${CSREQ_TMPDIR}/csreq.bin" >/dev/null; then
  fail "csreq failed for ${SIGN_TARGET}'s designated requirement"
fi
CSREQ_HEX="$(/usr/bin/od -An -tx1 -v "${CSREQ_TMPDIR}/csreq.bin" | /usr/bin/tr -d ' \n')"
[[ -n "${CSREQ_HEX}" ]] || fail "csreq produced an empty blob"

if [[ "$(/usr/bin/id -u)" -ne 0 ]]; then
  if /usr/bin/sudo -n -v 2>/dev/null; then
    true
  elif [[ "${READ_SUDO_PASSWORD}" == 1 ]]; then
    if IFS= read -r SUDO_PASSWORD_CACHE || [[ -n "${SUDO_PASSWORD_CACHE:-}" ]]; then
      true
    else
      fail "sudo password was not provided on stdin"
    fi
    printf '%s\n' "${SUDO_PASSWORD_CACHE}" | /usr/bin/sudo -S -p '' -v >/dev/null
  else
    fail "sudo is required to write ${TCC_DB}; pass the password through the host wrapper or run as root"
  fi
fi

[[ -f "${TCC_DB}" ]] || fail "system TCC.db does not exist at ${TCC_DB}"
ACCESS_COLUMNS="$(run_sudo /usr/bin/sqlite3 "${TCC_DB}" 'PRAGMA table_info(access);')"
CLIENT_SQL="$(sql_escape "${CLIENT}")"

if ! /usr/bin/grep -q '|auth_value|' <<< "${ACCESS_COLUMNS}"; then
  fail "unsupported TCC access schema; expected modern auth_value column"
fi

# Keep this row shape in sync with trycua/uvisor's TCCGrant.swift system grant
# path. Both helpers seed the same modern macOS TCC schema from a signed csreq.
SQL="
BEGIN;
INSERT OR REPLACE INTO access(
  service,client,client_type,auth_value,auth_reason,auth_version,
  csreq,flags,indirect_object_identifier_type,
  indirect_object_identifier,indirect_object_code_identity,last_modified
) VALUES
  ('kTCCServiceAccessibility','${CLIENT_SQL}',${CLIENT_TYPE},2,2,1,
   X'${CSREQ_HEX}',0,0,'UNUSED',NULL,strftime('%s','now')),
  ('kTCCServiceScreenCapture','${CLIENT_SQL}',${CLIENT_TYPE},2,2,1,
   X'${CSREQ_HEX}',0,0,'UNUSED',NULL,strftime('%s','now'));
COMMIT;
"
VERIFY_SQL="SELECT count(*) FROM access WHERE client='${CLIENT_SQL}' AND client_type=${CLIENT_TYPE} AND auth_value=2 AND service IN ('kTCCServiceAccessibility','kTCCServiceScreenCapture');"

run_sudo /usr/bin/sqlite3 "${TCC_DB}" "${SQL}"
ROW_COUNT="$(run_sudo /usr/bin/sqlite3 "${TCC_DB}" "${VERIFY_SQL}")"
[[ "${ROW_COUNT}" == 2 ]] || fail "expected two granted rows for ${CLIENT}, found ${ROW_COUNT}"

if run_sudo /usr/bin/pgrep -x tccd >/dev/null 2>&1; then
  run_sudo /usr/bin/killall tccd >/dev/null \
    || fail "seeded rows for ${CLIENT}, but failed to restart tccd; restart the VM or tccd before trusting permission status"
  TCCD_RESTARTED=1
else
  TCCD_RESTARTED=0
  echo "seed-tcc: warning: tccd was not running; no TCC cache process was restarted" >&2
fi

echo "seeded kTCCServiceAccessibility and kTCCServiceScreenCapture for ${CLIENT} (client_type=${CLIENT_TYPE})"
echo "restart CuaDriverLocal.app before checking permissions if it was already running"
echo "model: ${MODEL}"
echo "sip: ${SIP_STATUS}"
echo "csreq_bytes: $(( ${#CSREQ_HEX} / 2 ))"
echo "tccd_restarted: ${TCCD_RESTARTED}"
