#!/usr/bin/env bash
# Host-side helper: seed CuaDriverLocal.app Accessibility + Screen Recording
# in one or more running, SIP-disabled Lume macOS VMs.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GUEST_HELPER="${SCRIPT_DIR}/seed-tcc-guest.sh"
REMOTE_HELPER="/tmp/cua-driver-seed-tcc-guest.sh"
APP_PATH="/Applications/CuaDriverLocal.app"
BINARY_PATH=""
EXPECTED_CLIENT="com.trycua.driver.local"
SSH_USER="${LUME_SSH_USER:-lume}"
SSH_PASSWORD="${LUME_SSH_PASSWORD-${VM_PASSWORD:-lume}}"
SSH_TIMEOUT="${CUA_TCC_LUME_SSH_TIMEOUT:-120}"
SSH_KNOWN_HOSTS="${CUA_TCC_SSH_KNOWN_HOSTS:-/dev/null}"
STORAGE="${LUME_STORAGE:-}"
SUDO_PASSWORD="${CUA_TCC_SUDO_PASSWORD:-${VM_PASSWORD:-lume}}"
READ_SUDO_PASSWORD_STDIN=0
ALLOW_ADHOC=0
VMS=()
ASKPASS_DIR=""
ASKPASS_HELPER=""

cleanup_local() {
  if [[ -n "${ASKPASS_DIR}" && -d "${ASKPASS_DIR}" ]]; then
    /bin/rm -rf "${ASKPASS_DIR}"
  fi
}
trap cleanup_local EXIT

usage() {
  cat <<'USAGE'
Usage: seed-tcc.sh [options] <vm-name> [<vm-name> ...]

Seed macOS Accessibility and Screen Recording TCC rows for CuaDriverLocal.app
inside running, SSH-reachable, SIP-disabled Lume macOS VMs. This uses the
same SIP-off system-TCC seed model as the later uvisor `gui grant --gui`
path; it does not install the app. Run install-local first, then run this
from the host.

Options:
  --app PATH                 App bundle to grant inside the guest
                             (default: /Applications/CuaDriverLocal.app)
  --binary PATH              Executable to derive the code requirement from.
                             Defaults to CFBundleExecutable inside --app.
                             Only valid when --app names an app bundle.
  --expected-client ID       Expected TCC client identifier
                             (default: com.trycua.driver.local)
  --allow-adhoc              Allow cdhash-only ad-hoc signatures. The default
                             requires a certificate-backed local/release identity.
  --ssh-user USER            Lume SSH user (default: lume)
  --ssh-password PASSWORD    SSH password for direct ssh/scp after resolving
                             the Lume IP (default: LUME_SSH_PASSWORD,
                             VM_PASSWORD, or lume). Leave empty to use keys.
  --timeout SECONDS          Per-VM connect/copy/command timeout (default: 120)
  --storage STORAGE          Lume storage name/path to pass to lume get
  --sudo-password-stdin      Read the guest sudo password from stdin instead of
                             using CUA_TCC_SUDO_PASSWORD, VM_PASSWORD, or lume.
  -h, --help                 Show this help.

Example for two workers:
  libs/cua-driver/tests/runners/macos-lume/seed-tcc.sh \
    cua-driver-worker-a cua-driver-worker-b
USAGE
}

shell_quote() {
  printf '%q' "$1"
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
    --ssh-user)
      if (($# < 2)); then echo "--ssh-user requires a value" >&2; exit 2; fi
      SSH_USER="$2"; shift
      ;;
    --ssh-user=*) SSH_USER="${1#*=}" ;;
    --ssh-password)
      if (($# < 2)); then echo "--ssh-password requires a value" >&2; exit 2; fi
      SSH_PASSWORD="$2"; shift
      ;;
    --ssh-password=*) SSH_PASSWORD="${1#*=}" ;;
    --timeout)
      if (($# < 2)); then echo "--timeout requires a value" >&2; exit 2; fi
      SSH_TIMEOUT="$2"; shift
      ;;
    --timeout=*) SSH_TIMEOUT="${1#*=}" ;;
    --storage)
      if (($# < 2)); then echo "--storage requires a value" >&2; exit 2; fi
      STORAGE="$2"; shift
      ;;
    --storage=*) STORAGE="${1#*=}" ;;
    --sudo-password-stdin) READ_SUDO_PASSWORD_STDIN=1 ;;
    -h|--help) usage; exit 0 ;;
    --) shift; break ;;
    -*) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) VMS+=("$1") ;;
  esac
  shift
done

if (($#)); then
  VMS+=("$@")
fi

if ((${#VMS[@]} == 0)); then
  echo "at least one VM name is required" >&2
  usage >&2
  exit 2
fi

if [[ ! "${SSH_TIMEOUT}" =~ ^[0-9]+$ ]]; then
  echo "--timeout must be a non-negative integer" >&2
  exit 2
fi

command -v lume >/dev/null 2>&1 || {
  echo "lume is not on PATH" >&2
  exit 2
}
command -v ssh >/dev/null 2>&1 || {
  echo "ssh is not on PATH" >&2
  exit 2
}
command -v scp >/dev/null 2>&1 || {
  echo "scp is not on PATH" >&2
  exit 2
}
command -v python3 >/dev/null 2>&1 || {
  echo "python3 is not on PATH" >&2
  exit 2
}
if [[ "${SSH_TIMEOUT}" != 0 && ! -x /usr/bin/perl ]]; then
  echo "/usr/bin/perl is required to enforce --timeout; pass --timeout 0 to disable the command timeout" >&2
  exit 2
fi

if [[ ! -f "${GUEST_HELPER}" ]]; then
  echo "missing guest helper: ${GUEST_HELPER}" >&2
  exit 2
fi

if ((READ_SUDO_PASSWORD_STDIN)); then
  if IFS= read -r SUDO_PASSWORD || [[ -n "${SUDO_PASSWORD}" ]]; then
    true
  else
    echo "--sudo-password-stdin was set but stdin was empty" >&2
    exit 2
  fi
fi

resolve_lume_ip() {
  local vm="$1"
  local args=(get "${vm}" --format json)
  local info
  if [[ -n "${STORAGE}" ]]; then
    args+=(--storage "${STORAGE}")
  fi
  info="$(lume "${args[@]}")"
  python3 -c '
import json
import sys

name = sys.argv[1]
payload = json.load(sys.stdin)
vm = payload[0] if isinstance(payload, list) and payload else payload
status = vm.get("status")
ip = vm.get("ipAddress")
ssh_available = vm.get("sshAvailable")
if status != "running" or not ip:
    print(f"{name}: expected running VM with an IP, got status={status!r} ip={ip!r}", file=sys.stderr)
    sys.exit(1)
if ssh_available is False:
    print(f"{name}: SSH is not available yet", file=sys.stderr)
    sys.exit(1)
print(ip)
' "${vm}" <<< "${info}"
}

ssh_base_opts() {
  printf '%s\0' \
    -o StrictHostKeyChecking=no \
    -o "UserKnownHostsFile=${SSH_KNOWN_HOSTS}" \
    -o LogLevel=ERROR \
    -o "ConnectTimeout=${SSH_TIMEOUT}"
}

ensure_askpass_helper() {
  if [[ -n "${ASKPASS_HELPER}" ]]; then
    return
  fi
  ASKPASS_DIR="$(/usr/bin/mktemp -d /tmp/cua-driver-tcc-askpass.XXXXXX)"
  ASKPASS_HELPER="${ASKPASS_DIR}/askpass.sh"
  cat > "${ASKPASS_HELPER}" <<'ASKPASS'
#!/bin/sh
printf '%s\n' "$CUA_TCC_SSH_PASSWORD"
ASKPASS
  chmod 700 "${ASKPASS_HELPER}"
}

run_with_timeout() {
  if [[ "${SSH_TIMEOUT}" == 0 ]]; then
    "$@"
  else
    /usr/bin/perl -e 'alarm shift; exec @ARGV' "${SSH_TIMEOUT}" "$@"
  fi
}

run_ssh() {
  local ip="$1"
  local command="$2"
  local target="${SSH_USER}@${ip}"
  local opts=()
  local opt
  while IFS= read -r -d '' opt; do
    opts+=("${opt}")
  done < <(ssh_base_opts)
  if [[ -n "${SSH_PASSWORD}" ]]; then
    opts+=(-o PreferredAuthentications=password -o PubkeyAuthentication=no -o NumberOfPasswordPrompts=1)
    ensure_askpass_helper
    DISPLAY="${DISPLAY:-:0}" \
      SSH_ASKPASS="${ASKPASS_HELPER}" \
      SSH_ASKPASS_REQUIRE=force \
      CUA_TCC_SSH_PASSWORD="${SSH_PASSWORD}" \
      run_with_timeout ssh "${opts[@]}" "${target}" "${command}"
  else
    opts+=(-o BatchMode=yes)
    run_with_timeout ssh "${opts[@]}" "${target}" "${command}"
  fi
}

copy_to_guest() {
  local ip="$1"
  local source="$2"
  local dest="$3"
  local target="${SSH_USER}@${ip}:${dest}"
  local opts=()
  local opt
  while IFS= read -r -d '' opt; do
    opts+=("${opt}")
  done < <(ssh_base_opts)
  if [[ -n "${SSH_PASSWORD}" ]]; then
    opts+=(-o PreferredAuthentications=password -o PubkeyAuthentication=no -o NumberOfPasswordPrompts=1)
    ensure_askpass_helper
    DISPLAY="${DISPLAY:-:0}" \
      SSH_ASKPASS="${ASKPASS_HELPER}" \
      SSH_ASKPASS_REQUIRE=force \
      CUA_TCC_SSH_PASSWORD="${SSH_PASSWORD}" \
      run_with_timeout scp "${opts[@]}" "${source}" "${target}"
  else
    opts+=(-o BatchMode=yes)
    run_with_timeout scp "${opts[@]}" "${source}" "${target}"
  fi
}

for vm in "${VMS[@]}"; do
  ip="$(resolve_lume_ip "${vm}")"
  echo "[${vm}] installing guest TCC seeder"
  copy_to_guest "${ip}" "${GUEST_HELPER}" "${REMOTE_HELPER}"
  run_ssh "${ip}" "chmod 700 $(shell_quote "${REMOTE_HELPER}")"

  remote_cmd="CUA_TCC_APP_PATH=$(shell_quote "${APP_PATH}")"
  remote_cmd+=" CUA_TCC_BINARY_PATH=$(shell_quote "${BINARY_PATH}")"
  remote_cmd+=" CUA_TCC_EXPECTED_CLIENT=$(shell_quote "${EXPECTED_CLIENT}")"
  remote_cmd+=" CUA_TCC_ALLOW_ADHOC=${ALLOW_ADHOC}"
  remote_cmd+=" CUA_TCC_READ_SUDO_PASSWORD=1"
  remote_cmd+=" $(shell_quote "${REMOTE_HELPER}")"

  echo "[${vm}] seeding Accessibility + Screen Recording for ${EXPECTED_CLIENT}"
  set +e +o pipefail
  printf '%s\n' "${SUDO_PASSWORD}" | run_ssh "${ip}" "${remote_cmd}"
  pipe_status=("${PIPESTATUS[@]}")
  set -e -o pipefail
  ssh_status="${pipe_status[1]}"
  run_ssh "${ip}" "/bin/rm -f $(shell_quote "${REMOTE_HELPER}")" >/dev/null 2>&1 || true
  if [[ "${ssh_status}" -ne 0 ]]; then
    exit "${ssh_status}"
  fi
  echo "[${vm}] done"
done
