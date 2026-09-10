#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
APP="${1:?usage: build-embedded-menu-fixture.sh /absolute/path/Fixture.app unique.bundle.id}"
BUNDLE_ID="${2:?supply a unique test bundle identifier}"
[[ "$APP" = /*.app ]] || { printf '%s\n' 'app path must be absolute and end in .app' >&2; exit 1; }
[[ ! -e "$APP" ]] || { printf '%s\n' 'choose a new app path; existing apps are not overwritten' >&2; exit 1; }
BUILD_LOG="$(mktemp)"
trap 'rm -f "$BUILD_LOG"' EXIT

cargo test --manifest-path "$ROOT/libs/cua-driver/rust/Cargo.toml" \
    -p platform-macos --test embedded_menu_restore --locked --no-run \
    --message-format=json > "$BUILD_LOG"

python3 - "$BUILD_LOG" "$APP" "$BUNDLE_ID" <<'PY'
import json
import pathlib
import plistlib
import shutil
import sys

log, destination, identifier = sys.argv[1:]
artifacts = [json.loads(line) for line in pathlib.Path(log).read_text().splitlines() if line.startswith("{")]
executables = [item["executable"] for item in artifacts
               if item.get("reason") == "compiler-artifact"
               and item.get("target", {}).get("name") == "embedded_menu_restore"
               and item.get("executable")]
assert len(executables) == 1, executables
app = pathlib.Path(destination)
(app / "Contents" / "MacOS").mkdir(parents=True)
shutil.copy2(executables[0], app / "Contents" / "MacOS" / "embedded_menu_restore")
with (app / "Contents" / "Info.plist").open("wb") as output:
    plistlib.dump({
        "CFBundleIdentifier": identifier,
        "CFBundleName": app.stem,
        "CFBundleDisplayName": app.stem,
        "CFBundleExecutable": "embedded_menu_restore",
        "CFBundlePackageType": "APPL",
        "CFBundleVersion": "1",
        "CFBundleShortVersionString": "1.0",
        "NSHighResolutionCapable": True,
    }, output)
PY

codesign --force --sign - --identifier "$BUNDLE_ID" "$APP"
codesign --verify --strict "$APP"
printf 'fixture app: %s\nbundle identifier: %s\n' "$APP" "$BUNDLE_ID"
