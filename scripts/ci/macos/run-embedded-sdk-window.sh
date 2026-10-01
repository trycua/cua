#!/usr/bin/env bash
# One embedded SDK fixture in an already-authorized GUI session. No permission setup.
set -euo pipefail
set +x
export CUA_E2E_RUNNER_LIB_ONLY=1
export CUA_E2E_REPO_ROOT="$PWD"
source libs/cua-driver/tests/runners/macos-lume/run-all.sh
unset CUA_E2E_RUNNER_LIB_ONLY
FIXTURE_DIR="$PWD/artifacts/cua-driver/embedded-sdk-window"
mkdir -p "$FIXTURE_DIR"
source_sha="$(tr -d '[:space:]' < .cua-e2e-source-sha)"
[[ "$source_sha" =~ ^[0-9a-f]{40}$ ]] || exit 2
binary=""
identity=""
binary_before=""
signature_verified=false
finish_fixture() {
  local fixture_exit=$?
  trap - EXIT
  unset CUA_E2E_SIGNING_KEYCHAIN_PASSWORD
  if ! "$HOME/.local/bin/cua-driver-local" status > "$FIXTURE_DIR/daemon-after.txt" 2>&1; then
    fixture_exit=1
  fi
  python3 - "$FIXTURE_DIR" "$source_sha" "$fixture_exit" "$binary" "$identity" "$binary_before" "$signature_verified" <<'PY'
import hashlib, json, pathlib, sys
root, sha, code, binary, identity, before, verified = sys.argv[1:]
path = pathlib.Path(binary) if binary else None
report = {
    'fixture': 'embedded-sdk-window', 'source_sha': sha, 'exit_code': int(code),
    'executable': binary, 'binary_sha256_before': before,
    'binary_sha256_after': hashlib.sha256(path.read_bytes()).hexdigest() if path and path.is_file() else None,
    'signing': {'identity': identity, 'identifier': 'com.trycua.fixture.embedded-sdk-window', 'verified': verified == 'true'},
}
pathlib.Path(root, 'runner-manifest.json').write_text(json.dumps(report, indent=2) + '\n')
PY
  exit "$fixture_exit"
}
trap finish_fixture EXIT
[[ -z "${SSH_CONNECTION:-}" && -z "${SSH_TTY:-}" ]]
[[ "$(stat -f '%Su' /dev/console)" == "$(id -un)" ]]
"$HOME/.local/bin/cua-driver-local" status > "$FIXTURE_DIR/daemon-before.txt"
[[ "$(cat "$FIXTURE_DIR/daemon-before.txt")" == *'permission mode: standard'* ]]
[[ -f "$SIGNING_KEYCHAIN" && -n "${CUA_E2E_SIGNING_KEYCHAIN_PASSWORD:-}" ]] || {
  echo 'Missing existing dedicated signing keychain or canonical unlock prerequisite' >&2
  exit 2
}
prepare_keychain 'Dedicated signing keychain' "$SIGNING_KEYCHAIN" "$CUA_E2E_SIGNING_KEYCHAIN_PASSWORD" signing
unset CUA_E2E_SIGNING_KEYCHAIN_PASSWORD
capture_command_output run_bounded_command security find-identity -v -p codesigning "$SIGNING_KEYCHAIN"
identity="$(python3 - "$SIGNING_CN" "$CAPTURED_OUTPUT" <<'PY'
import re, sys
matches = re.findall(r'\b([0-9A-F]{40}) "' + re.escape(sys.argv[1]) + r'"', sys.argv[2])
if len(matches) != 1:
    raise SystemExit('Expected exactly one existing configured signing identity')
print(matches[0])
PY
)"
export CUA_E2E_SIGNING_IDENTITY="$identity"
probe_signing_keychain "$SIGNING_KEYCHAIN"
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR
CARGO_TARGET_DIR="$(resolve_cargo_target_dir "$source_sha" "$(basename "$(dirname "$PWD")")")"
cargo test --manifest-path libs/cua-driver/rust/Cargo.toml --locked -p platform-macos \
  --test embedded_menu_restore --no-run --message-format=json > "$FIXTURE_DIR/build.jsonl"
binary="$(python3 - "$FIXTURE_DIR/build.jsonl" <<'PY'
import json, pathlib, sys
rows = [json.loads(line) for line in pathlib.Path(sys.argv[1]).read_text().splitlines()]
paths = [row['executable'] for row in rows if row.get('reason') == 'compiler-artifact'
         and row.get('target', {}).get('name') == 'embedded_menu_restore' and row.get('executable')]
if len(paths) != 1:
    raise SystemExit('Expected exactly one built embedded SDK executable')
print(pathlib.Path(paths[0]).resolve())
PY
)"
run_bounded_command codesign --force --timestamp=none --sign "$identity" --keychain "$SIGNING_KEYCHAIN" \
  --identifier com.trycua.fixture.embedded-sdk-window "$binary"
run_bounded_command codesign --verify --strict "$binary"
codesign -d --verbose=4 "$binary" > "$FIXTURE_DIR/signature.txt" 2>&1
[[ "$(cat "$FIXTURE_DIR/signature.txt")" == *'Identifier=com.trycua.fixture.embedded-sdk-window'* ]]
codesign -d --extract-certificates="$FIXTURE_DIR/signing-cert" "$binary"
[[ "$(shasum "$FIXTURE_DIR/signing-cert0" | cut -d ' ' -f 1 | tr '[:lower:]' '[:upper:]')" == "$identity" ]]
signature_verified=true
binary_before="$(shasum -a 256 "$binary" | cut -d ' ' -f 1)"
export CUA_E2E_SOURCE_SHA="$source_sha"
# Execute the actual signed process as a Terminal descendant. Its AX trust is observed,
# never inferred from the certificate or from CuaDriverLocal's unrelated grants.
run_bounded_command "$binary" --status-only --evidence "$FIXTURE_DIR/preflight.json"
run_fixture() {
  local KEYCHAIN_COMMAND_TIMEOUT_SECONDS=120
  run_bounded_command "$binary" --run-gui --report "$FIXTURE_DIR/fixture.txt" --evidence "$FIXTURE_DIR/fixture.json"
}
run_fixture
