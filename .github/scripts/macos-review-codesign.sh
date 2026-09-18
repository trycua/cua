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
keychain_created=false
search_list_snapshotted=false
previous_keychains=()
previous_keychain_count=0

cleanup() {
  local status=$?
  trap - EXIT INT TERM
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
  rm -f "$private_key_path" "$certificate_path" "$certificate_der_path" \
    "$identity_path" "$keychain_path"
  rmdir "$work_root" >/dev/null 2>&1 || true
  keychain_password=""
  identity_password=""
  exit "$status"
}
trap cleanup EXIT INT TERM

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
  -subj "/CN=Cua Review Candidate ${GITHUB_RUN_ID:-local}/O=Cua Review Only" \
  -addext "basicConstraints=critical,CA:FALSE" \
  -addext "keyUsage=critical,digitalSignature" \
  -addext "extendedKeyUsage=codeSigning" \
  -keyout "$private_key_path" -out "$certificate_path"
openssl x509 -in "$certificate_path" -outform der -out "$certificate_der_path"
openssl pkcs12 -export -keypbe PBE-SHA1-3DES -certpbe PBE-SHA1-3DES -macalg sha1 \
  -inkey "$private_key_path" -in "$certificate_path" \
  -name "Cua Review Candidate" -passout "pass:$identity_password" -out "$identity_path"

security create-keychain -p "$keychain_password" "$keychain_path"
keychain_created=true
security set-keychain-settings -lut 900 "$keychain_path"
security unlock-keychain -p "$keychain_password" "$keychain_path"
if (( previous_keychain_count > 0 )); then
  security list-keychains -d user -s "$keychain_path" "${previous_keychains[@]}"
else
  security list-keychains -d user -s "$keychain_path"
fi
security import "$identity_path" -k "$keychain_path" -P "$identity_password" \
  -T /usr/bin/codesign >/dev/null
security set-key-partition-list -S apple-tool:,apple:,codesign: -s \
  -k "$keychain_password" "$keychain_path" >/dev/null

identities="$({ security find-identity -p codesigning "$keychain_path" || true; } |
  sed -nE 's/^[[:space:]]*[0-9]+\) ([0-9A-F]{40}) .*/\1/p')"
[[ "$(printf '%s\n' "$identities" | sed '/^$/d' | wc -l | tr -d ' ')" == 1 ]] || {
  echo "temporary keychain must contain exactly one code-signing identity" >&2
  exit 1
}
identity_hash="$identities"
codesign --force --sign "$identity_hash" --keychain "$keychain_path" "$driver_path"
codesign --verify --strict --verbose=2 "$driver_path"

requirement_output="$({ codesign -d -r- "$driver_path"; } 2>&1)"
requirement="$(printf '%s\n' "$requirement_output" | sed -n 's/^designated => //p')"
printf '%s\n' "$requirement" |
  grep -Eq 'certificate (leaf|root) = H"[[:xdigit:]]{40}"' || {
  echo "review Driver designated requirement is not certificate-backed" >&2
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
