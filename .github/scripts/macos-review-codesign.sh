#!/usr/bin/env bash

set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 DRIVER_PATH MEASUREMENT_PATH" >&2
  exit 2
fi
if [[ "$(uname -s)" != Darwin ]]; then
  echo "macOS review signing must run on Darwin" >&2
  exit 2
fi

driver_path="$1"
measurement_path="$2"
[[ -f "$driver_path" ]] || { echo "review Driver is missing: $driver_path" >&2; exit 2; }
[[ ! -e "$measurement_path" ]] || { echo "measurement output already exists: $measurement_path" >&2; exit 2; }

work_root="$(mktemp -d "${RUNNER_TEMP:-/tmp}/cua-review-codesign.XXXXXX")"
keychain_path="$work_root/review-signing.keychain-db"
private_key_path="$work_root/review-signing.key"
certificate_path="$work_root/review-signing.crt"
certificate_der_path="$work_root/review-signing.der"
identity_path="$work_root/review-signing.p12"
keychain_password="$(openssl rand -hex 32)"
identity_password="$(openssl rand -hex 32)"
identity_name="Cua Review Candidate ${GITHUB_RUN_ID:-local}"
keychain_created=false
search_list_snapshotted=false
default_keychain_snapshotted=false
previous_keychains=()
previous_keychain_count=0
previous_default_keychain=""
trusted_identity=""
admin_trust_cleanup_needed=false
log_dir="$work_root/logs"
signing_probe="$work_root/signing-probe"

run_bounded() {
  /usr/bin/perl -e 'alarm shift; exec @ARGV' 60 "$@"
}

emit_log() {
  sed -E 's/[[:xdigit:]]{64}/<redacted>/g' "$1" >&2
}

run_step() {
  local label="$1"
  shift
  if ! run_bounded "$@" >"$log_dir/$label.log" 2>&1; then
    echo "review signing step failed: $label" >&2
    emit_log "$log_dir/$label.log"
    exit 1
  fi
}

cleanup() {
  local status=$?
  local cleanup_status="$status"
  local cleanup_failed=false
  trap - EXIT INT TERM
  set +e
  if [[ "$admin_trust_cleanup_needed" == true \
      && "$trusted_identity" =~ ^[0-9A-Fa-f]{40}$ ]]; then
    run_bounded sudo -n security remove-trusted-cert -d "$certificate_path" \
      >/dev/null 2>&1 || true
    run_bounded sudo -n security delete-certificate -Z "$trusted_identity" \
      /Library/Keychains/System.keychain >/dev/null 2>&1 || true
    system_certificates="$(run_bounded security find-certificate -Z -a \
      /Library/Keychains/System.keychain 2>&1)"
    system_certificates_status=$?
    if [[ "$system_certificates_status" != 0 ]]; then
      echo "cannot verify removal of the temporary system certificate" >&2
      cleanup_failed=true
    elif [[ "$system_certificates" == *"$trusted_identity"* ]]; then
      echo "temporary review certificate remained in the system keychain" >&2
      cleanup_failed=true
    fi
    admin_trust_dump="$(run_bounded security dump-trust-settings -d 2>&1)"
    admin_trust_status=$?
    if [[ "$admin_trust_status" != 0 \
        && "$admin_trust_dump" != *"No Trust Settings were found."* ]]; then
      echo "cannot verify removal of temporary admin trust settings" >&2
      cleanup_failed=true
    fi
    if [[ "$admin_trust_dump" == *"Cua Review Candidate ${GITHUB_RUN_ID:-local}"* ]]; then
      echo "temporary review trust setting remained configured" >&2
      cleanup_failed=true
    fi
  fi
  if [[ "$default_keychain_snapshotted" == true && -n "$previous_default_keychain" ]]; then
    security default-keychain -d user -s "$previous_default_keychain" \
      >/dev/null 2>&1 || true
  fi
  if [[ "$search_list_snapshotted" == true ]]; then
    if (( previous_keychain_count > 0 )); then
      security list-keychains -d user -s "${previous_keychains[@]}" >/dev/null 2>&1 || true
    else
      security list-keychains -d user -s >/dev/null 2>&1 || true
    fi
  fi
  if [[ "$keychain_created" == true ]]; then
    security delete-keychain "$keychain_path" >/dev/null 2>&1 || true
  fi
  rm -f "$signing_probe" \
    "$log_dir/import.log" "$log_dir/partition.log" \
    "$log_dir/pkcs12-legacy.log" "$log_dir/pkcs12-fallback.log" \
    "$log_dir/identity.log" "$log_dir/valid-identity.log" \
    "$log_dir/sudo-preflight.log" "$log_dir/add-trust.log" \
    "$log_dir/trusted-cert-present.log" "$log_dir/probe-sign-name.log" \
    "$log_dir/probe-sign-hash.log" "$log_dir/probe-verify.log" \
    "$log_dir/driver-sign.log" \
    "$log_dir/driver-verify.log"
  rmdir "$log_dir" >/dev/null 2>&1 || true
  rm -f "$private_key_path" "$certificate_path" "$certificate_der_path" \
    "$identity_path" "$keychain_path"
  rmdir "$work_root" >/dev/null 2>&1 || true
  keychain_password=""
  identity_password=""
  if [[ "$cleanup_failed" == true && "$status" == 0 ]]; then
    cleanup_status=1
  fi
  exit "$cleanup_status"
}
trap cleanup EXIT INT TERM

mkdir -p "$log_dir"

previous_default_keychain="$(security default-keychain -d user |
  sed -E 's/^[[:space:]]*"//; s/"[[:space:]]*$//')"
[[ -n "$previous_default_keychain" ]] || {
  echo "cannot determine the current user default keychain" >&2
  exit 1
}
default_keychain_snapshotted=true

previous_keychains_output="$(security list-keychains -d user)"
while IFS= read -r listed_keychain; do
  listed_keychain="${listed_keychain#*\"}"
  listed_keychain="${listed_keychain%\"*}"
  if [[ -n "$listed_keychain" ]]; then
    previous_keychains+=("$listed_keychain")
    previous_keychain_count=$((previous_keychain_count + 1))
  fi
done <<< "$previous_keychains_output"
unset previous_keychains_output
search_list_snapshotted=true

openssl req -new -newkey rsa:2048 -nodes -x509 -sha256 -days 2 \
  -subj "/CN=$identity_name/O=Cua Review Only" \
  -addext "basicConstraints=critical,CA:FALSE" \
  -addext "keyUsage=critical,digitalSignature" \
  -addext "extendedKeyUsage=codeSigning" \
  -keyout "$private_key_path" -out "$certificate_path"
openssl x509 -in "$certificate_path" -outform der -out "$certificate_der_path"
if ! openssl pkcs12 -export -legacy \
    -inkey "$private_key_path" -in "$certificate_path" \
    -name "$identity_name" -passout "pass:$identity_password" \
    -out "$identity_path" >"$log_dir/pkcs12-legacy.log" 2>&1; then
  if ! openssl pkcs12 -export \
      -inkey "$private_key_path" -in "$certificate_path" \
      -name "$identity_name" -passout "pass:$identity_password" \
      -out "$identity_path" >"$log_dir/pkcs12-fallback.log" 2>&1; then
    echo "review signing step failed: pkcs12-export" >&2
    emit_log "$log_dir/pkcs12-legacy.log"
    emit_log "$log_dir/pkcs12-fallback.log"
    exit 1
  fi
fi

security create-keychain -p "$keychain_password" "$keychain_path"
keychain_created=true
security set-keychain-settings -lut 900 "$keychain_path"
security unlock-keychain -p "$keychain_password" "$keychain_path"
if (( previous_keychain_count > 0 )); then
  security list-keychains -d user -s "$keychain_path" "${previous_keychains[@]}"
else
  security list-keychains -d user -s "$keychain_path"
fi
security default-keychain -d user -s "$keychain_path"

# This allow-all ACL is confined to a generated key in an ephemeral keychain on
# a single-tenant runner. The EXIT trap deletes both the key and its keychain.
run_step import security import "$identity_path" -k "$keychain_path" \
  -P "$identity_password" -A -T /usr/bin/codesign

run_step identity security find-identity -p codesigning "$keychain_path"
identities="$(sed -nE 's/^[[:space:]]*[0-9]+\) ([0-9A-F]{40}) .*/\1/p' \
  "$log_dir/identity.log")"
[[ "$(printf '%s\n' "$identities" | sed '/^$/d' | wc -l | tr -d ' ')" == 1 ]] || {
  echo "temporary keychain must contain exactly one code-signing identity" >&2
  emit_log "$log_dir/identity.log"
  exit 1
}
identity_hash="$identities"
trusted_identity="$identity_hash"

if [[ "${GITHUB_ACTIONS:-}" == true \
    && "${RUNNER_ENVIRONMENT:-}" == github-hosted \
    && "${RUNNER_OS:-}" == macOS ]]; then
  # GitHub's macOS 15 image resolves the imported key for codesign only after
  # the self-signed certificate has a temporary code-signing trust setting.
  run_step sudo-preflight sudo -n -v
  admin_trust_cleanup_needed=true
  run_step add-trust sudo -n security add-trusted-cert -d -r trustRoot \
    -p codeSign -k /Library/Keychains/System.keychain "$certificate_path"
  run_step trusted-cert-present security find-certificate -Z -a \
    /Library/Keychains/System.keychain
  grep -Fqi "$identity_hash" "$log_dir/trusted-cert-present.log" || {
    echo "temporary review certificate was not found in the system keychain" >&2
    emit_log "$log_dir/trusted-cert-present.log"
    exit 1
  }
  run_step valid-identity security find-identity -v -p codesigning "$keychain_path"
  grep -Fqi "$identity_hash" "$log_dir/valid-identity.log" || {
    echo "temporary code-signing identity did not become valid" >&2
    emit_log "$log_dir/valid-identity.log"
    exit 1
  }
fi

# Some macOS 15 runners cannot update partitions on a freshly imported key.
# The signing probe below is the authoritative noninteractive usability check.
if ! run_bounded security set-key-partition-list \
    -S apple-tool:,apple:,codesign: -s -k "$keychain_password" "$keychain_path" \
    >"$log_dir/partition.log" 2>&1; then
  echo "note: set-key-partition-list did not apply; relying on the import ACL" >&2
  emit_log "$log_dir/partition.log"
fi

# Identity discovery proves presence; this probe proves the private key can sign
# without a GUI authorization prompt before the candidate is modified. Prefer
# the unique certificate name and default keychain path used by release signing;
# retain the exact certificate hash as a compatibility fallback.
cp /bin/echo "$signing_probe"
identity_selector="$identity_name"
if ! run_bounded codesign --force --sign "$identity_selector" "$signing_probe" \
    >"$log_dir/probe-sign-name.log" 2>&1; then
  identity_selector="$identity_hash"
  if ! run_bounded codesign --force --sign "$identity_selector" "$signing_probe" \
      >"$log_dir/probe-sign-hash.log" 2>&1; then
    echo "review signing step failed: probe-sign" >&2
    emit_log "$log_dir/probe-sign-name.log"
    emit_log "$log_dir/probe-sign-hash.log"
    exit 1
  fi
fi
run_step probe-verify codesign --verify --strict "$signing_probe"

run_step driver-sign codesign --force --sign "$identity_selector" "$driver_path"
run_step driver-verify codesign --verify --strict --verbose=2 "$driver_path"

requirement_output="$({ codesign -d -r- "$driver_path"; } 2>&1)"
requirement="$(printf '%s\n' "$requirement_output" | sed -n 's/^designated => //p')"
printf '%s\n' "$requirement" |
  grep -Eqi "certificate (leaf|root) = H\"$identity_hash\"" || {
  echo "review Driver designated requirement does not bind the selected certificate" >&2
  exit 1
}
certificate_sha256="$(shasum -a 256 "$certificate_der_path" | awk '{print $1}')"

CODE_SIGNING_REQUIREMENT="$requirement" \
CODE_SIGNING_CERTIFICATE_SHA256="$certificate_sha256" \
python3 - "$measurement_path" <<'PY'
import json
import os
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
value = {
    "status": "verified",
    "format": "apple-codesign",
    "identity": "ephemeral-self-signed-review-only",
    "certificate_sha256": os.environ["CODE_SIGNING_CERTIFICATE_SHA256"],
    "designated_requirement": os.environ["CODE_SIGNING_REQUIREMENT"],
}
path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")
PY
