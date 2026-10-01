#!/usr/bin/env bash
# One embedded SDK fixture in an already-authorized GUI session. No permission setup.
set -euo pipefail
set +x
ACCESSIBILITY_HANDOFF=0
if (($#)); then
  [[ "$#" == 1 && "$1" == --accessibility-handoff ]] || exit 2
  ACCESSIBILITY_HANDOFF=1
fi
export CUA_E2E_RUNNER_LIB_ONLY=1
export CUA_E2E_REPO_ROOT="$PWD"
source libs/cua-driver/tests/runners/macos-lume/run-all.sh
unset CUA_E2E_RUNNER_LIB_ONLY
FIXTURE_DIR="$PWD/artifacts/cua-driver/embedded-sdk-window"
mkdir -p "$FIXTURE_DIR"
source_sha="$(tr -d '[:space:]' < .cua-e2e-source-sha)"
[[ "$source_sha" =~ ^[0-9a-f]{40}$ ]] || exit 2
APP="$HOME/Applications/Cua Embedded SDK Window Fixture.app"
bundle_info_before=""
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
  python3 - "$FIXTURE_DIR" "$source_sha" "$fixture_exit" "$binary" "$identity" "$binary_before" "$signature_verified" "$APP" "$bundle_info_before" "$ACCESSIBILITY_HANDOFF" <<'PY'
import hashlib, json, pathlib, sys
root, sha, code, binary, identity, before, verified, app, info_before, handoff = sys.argv[1:]
info = pathlib.Path(app, "Contents/Info.plist")
path = pathlib.Path(binary) if binary else None
report = {
    'fixture': 'embedded-sdk-window', 'source_sha': sha, 'exit_code': int(code),
    'executable': binary, 'binary_sha256_before': before,
    'bundle_path': app, 'launch_method': 'LaunchServices', 'accessibility_handoff': handoff == '1',
    'bundle_info_sha256_before': info_before,
    'bundle_info_sha256_after': hashlib.sha256(info.read_bytes()).hexdigest() if info.is_file() else None,
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
# Match the existing fixture bundle layout, with the reviewed stable signer.
python3 - "$binary" "$APP" <<'PYBUNDLE'
import pathlib, plistlib, shutil, sys
binary, destination = sys.argv[1:]
app = pathlib.Path(destination)
if app.exists():
    raise SystemExit('Refusing to overwrite an existing fixture app')
(app / 'Contents/MacOS').mkdir(parents=True)
shutil.copy2(binary, app / 'Contents/MacOS/embedded_menu_restore')
with (app / 'Contents/Info.plist').open('wb') as stream:
    plistlib.dump({'CFBundleIdentifier': 'com.trycua.fixture.embedded-sdk-window',
                  'CFBundleName': 'Cua Embedded SDK Window Fixture',
                  'CFBundleDisplayName': 'Cua Embedded SDK Window Fixture',
                  'CFBundleExecutable': 'embedded_menu_restore', 'CFBundlePackageType': 'APPL',
                  'CFBundleVersion': '1', 'CFBundleShortVersionString': '1.0',
                  'NSHighResolutionCapable': True}, stream)
PYBUNDLE
binary="$APP/Contents/MacOS/embedded_menu_restore"
run_bounded_command codesign --force --timestamp=none --sign "$identity" --keychain "$SIGNING_KEYCHAIN" "$APP"
run_bounded_command codesign --verify --strict "$APP"
codesign -d --verbose=4 "$APP" > "$FIXTURE_DIR/signature.txt" 2>&1
[[ "$(cat "$FIXTURE_DIR/signature.txt")" == *'Identifier=com.trycua.fixture.embedded-sdk-window'* ]]
[[ "$(cat "$FIXTURE_DIR/signature.txt")" != *'Info.plist=not bound'* ]]
codesign -d --extract-certificates="$FIXTURE_DIR/signing-cert" "$binary"
[[ "$(shasum "$FIXTURE_DIR/signing-cert0" | cut -d ' ' -f 1 | tr '[:lower:]' '[:upper:]')" == "$identity" ]]
signature_verified=true
binary_before="$(shasum -a 256 "$binary" | cut -d ' ' -f 1)"
cp "$APP/Contents/Info.plist" "$FIXTURE_DIR/bundle-info.plist"
bundle_info_before="$(shasum -a 256 "$APP/Contents/Info.plist" | cut -d ' ' -f 1)"
export CUA_E2E_SOURCE_SHA="$source_sha"

launch_fixture_app() {
  local KEYCHAIN_COMMAND_TIMEOUT_SECONDS="$1" phase="$2"
  shift 2
  # Every caller ends its arguments with --evidence and a new report path.
  local report="${@: -1}" mode="$1"
  [[ ! -e "$report" && ! -L "$report" ]] || {
    echo 'Refusing existing fixture report' >&2
    return 2
  }
  # Do not use open -W: a short-lived app can exit before its kevent registration.
  run_bounded_command /usr/bin/open -n -a "$APP" \
    --env "CUA_E2E_SOURCE_SHA=$source_sha" --stdout "$FIXTURE_DIR/$phase.stdout" \
    --stderr "$FIXTURE_DIR/$phase.stderr" --args "$@" || return 2
  python3 - "$report" "$source_sha" "$APP" "$binary" "$mode" "$KEYCHAIN_COMMAND_TIMEOUT_SECONDS" <<'PYWAIT'
import json, os, pathlib, sys, time
path, sha, app, binary, mode, timeout = sys.argv[1:]
deadline = time.monotonic() + float(timeout)
expected = 'preflight' if mode == '--status-only' else 'pass'
pid = None
while time.monotonic() < deadline:
    try:
        r = json.loads(pathlib.Path(path).read_text())
    except (FileNotFoundError, json.JSONDecodeError):
        # The fixture writes in place; never accept a partial or absent report.
        time.sleep(0.1)
        continue
    try:
        p = r['process']
        assert r['schema'] == 'cua-driver/embedded-sdk-window@1' and r['source_sha'] == sha
        assert p['bundle_path'] == app and p['bundle_id'] == 'com.trycua.fixture.embedded-sdk-window'
        assert p['executable'] == binary and p['appkit_main_thread'] is True
        assert type(p['pid']) is int and p['pid'] > 0 and type(p['ax_trusted']) is bool
        assert pid is None or pid == p['pid']
        pid = p['pid']
        assert r['status'] in (expected, 'running')
    except (AssertionError, KeyError, TypeError):
        print('Invalid fixture launch report', file=sys.stderr)
        sys.exit(2)
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        # Read again after exit: the terminal write can race the earlier read.
        final = json.loads(pathlib.Path(path).read_text())
        if final == r and r['status'] == expected:
            sys.exit(0)
        if final == r:
            print('Fixture exited without a successful terminal report', file=sys.stderr)
            sys.exit(2)
        continue
    time.sleep(0.1)
print('Timed out waiting for fresh fixture report and process exit', file=sys.stderr)
sys.exit(2)
PYWAIT
}

read_preflight_trust() {
  python3 - "$FIXTURE_DIR/preflight.json" "$source_sha" "$APP" "$binary" <<'PYTRUST'
import json, pathlib, sys
path, sha, app, binary = sys.argv[1:]
r = json.loads(pathlib.Path(path).read_text())
p = r['process']
assert (r['schema'], r['source_sha'], r['status']) == ('cua-driver/embedded-sdk-window@1', sha, 'preflight')
assert p['bundle_path'] == app and p['bundle_id'] == 'com.trycua.fixture.embedded-sdk-window'
assert p['executable'] == binary and p['appkit_main_thread'] is True
assert type(p['pid']) is int and p['pid'] > 0 and type(p['ax_trusted']) is bool
print('true' if p['ax_trusted'] else 'false')
PYTRUST
}

record_handoff() {
  python3 - "$FIXTURE_DIR" "$ACCESSIBILITY_HANDOFF" "$1" <<'PYHANDOFF'
import json, pathlib, sys
root, requested, completed = sys.argv[1:]
root = pathlib.Path(root)
initial = json.loads((root / 'preflight-initial.json').read_text())['process']
latest = json.loads((root / 'preflight.json').read_text())['process']
(root / 'handoff.json').write_text(json.dumps({'requested': requested == '1',
    'needed': initial['ax_trusted'] is False, 'completed': completed == 'true',
    'initial_pid': initial['pid'], 'trusted_pid': latest['pid'] if latest['ax_trusted'] else None}, indent=2) + '\n')
PYHANDOFF
}

ensure_fixture_trust() {
  local probe=0 deadline=$((SECONDS + 300)) trusted
  while true; do
    # New process and new report every time: open's exit code does not prove AX trust.
    local report="$FIXTURE_DIR/preflight-$probe.json"
    [[ ! -e "$report" ]] || return 2
    launch_fixture_app 15 "preflight-$probe" --status-only --evidence "$report" || return 2
    cp "$report" "$FIXTURE_DIR/preflight.json" || return 2
    trusted="$(read_preflight_trust)" || return 2
    if ((probe == 0)); then cp "$report" "$FIXTURE_DIR/preflight-initial.json"; fi
    record_handoff "$trusted" || return 2
    if [[ "$trusted" == true ]]; then return 0; fi
    if ((ACCESSIBILITY_HANDOFF != 1)); then
      echo 'Fixture app lacks AX trust; no handoff was authorized and no permission was requested.' >&2
      return 2
    fi
    if ((SECONDS >= deadline)); then
      echo 'App-only Accessibility handoff expired without verified trust.' >&2
      return 2
    fi
    echo "Waiting for authorized app-only Accessibility handoff: $APP"
    sleep 5
    probe=$((probe + 1))
  done
}

ensure_fixture_trust
launch_fixture_app 120 native --run-gui --report "$FIXTURE_DIR/fixture.txt" --evidence "$FIXTURE_DIR/fixture.json"
python3 - "$FIXTURE_DIR" <<'PYRESULT'
import json, pathlib, sys
root = pathlib.Path(sys.argv[1])
preflight = json.loads((root / 'preflight.json').read_text())
result = json.loads((root / 'fixture.json').read_text())
assert result['status'] == 'pass' and result['source_sha'] == preflight['source_sha']
assert result['process']['pid'] != preflight['process']['pid']
for key in ('executable', 'bundle_path', 'bundle_id', 'ax_trusted', 'appkit_main_thread'):
    assert result['process'][key] == preflight['process'][key]
PYRESULT
