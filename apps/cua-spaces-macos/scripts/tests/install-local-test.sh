#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Tests scripts/install-local.sh and scripts/uninstall-local.sh (CUA-1776)
# against a fake HOME and a fake build. Nothing here builds Cua Spaces, touches
# the real HOME, the real Keychain, launchd, LaunchServices or TCC (the tools
# that would are replaced by logging shims). Signing is ad hoc unless you opt in
# to the certificate checks (see "signing" below). It checks, in one place, that:
#
#   - a Stable install (~/.cua, ~/.lume, ~/.local/bin/cua, the Stable app and
#     its support folder, launchd jobs, Cua Volume) is byte-identical before
#     and after install-local.sh, a rebuild and uninstall-local.sh;
#   - install-local.sh adds only the Local app and `cua-local`;
#   - a bundle at the Local path with another id is refused, and so is a build
#     that carries a Stable identity, a feed or the `cua` scheme;
#   - a copy that fails to verify is rolled back to the previous app;
#   - uninstall-local.sh removes the Local pieces and only those;
#   - the names in scripts/_local-common.sh are the ones CuaChannel::Local has.
#
#   scripts/tests/install-local-test.sh
#
# Each check prints PASS or FAIL; the exit status is the number of FAILs.
# The checks are strings for eval, and the shims are written from strings: the
# variables they read are assigned here, and shellcheck cannot see that.
# shellcheck disable=SC2034,SC2016,SC2030,SC2031,SC2119,SC2120,SC2329
set -uo pipefail

[ "$(uname -s)" = Darwin ] || { echo "SKIP: macOS only"; exit 0; }
command -v cc >/dev/null 2>&1 || { echo "SKIP: needs a C compiler (Xcode command line tools)"; exit 0; }

TESTS_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
SCRIPTS_DIR="$(cd -- "$TESTS_DIR/.." && pwd)"
REPO_ROOT="$(cd -- "$SCRIPTS_DIR/../../.." && pwd)"
CHANNEL_RS="$REPO_ROOT/libs/cua/crates/cua-home/src/channel.rs"
SIGNING_LIB="$REPO_ROOT/libs/cua-driver/scripts/_local-signing.sh"

fails=0
pass() { printf 'PASS  %s\n' "$*"; }
fail() { printf 'FAIL  %s\n' "$*"; fails=$((fails + 1)); }
check() { if eval "$2"; then pass "$1"; else fail "$1"; fi; }
note() { printf '      %s\n' "$*"; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/cua-install-local-test.XXXXXX")"
WORK="$(cd "$WORK" && pwd -P)"
trap 'rm -rf "$WORK"' EXIT

# --- The fixture: a repo tree with the real scripts and a fake build -------------

FAKE_REPO="$WORK/repo"
FAKE_SCRIPTS="$FAKE_REPO/apps/cua-spaces-macos/scripts"
FAKE_HOME="$WORK/home"
SHIMS="$WORK/shims"
SHIM_STATE="$WORK/shim-state"
SHIM_LOG="$WORK/shim.log"
STUB_LOG="$WORK/stub.log"
BUILD_LOG="$WORK/build.log"
LUME_LOG="$WORK/lume.log"
STUB_BIN="$WORK/stub"
STUB_DYLIB="$WORK/libfake.dylib"
CARGO_LOG="$WORK/cargo.log"
# The cua-spacesd this checkout builds, and the pinned release build-app.sh
# bundles unless it is told otherwise: two programs with different code.
DAEMON_CHECKOUT="$WORK/daemon-checkout"
DAEMON_PINNED="$WORK/daemon-pinned"
FAKE_DAEMON_BUILT="$FAKE_REPO/libs/cua-spacesd/target/release/cua-spacesd"

mkdir -p "$FAKE_SCRIPTS" "$FAKE_REPO/libs/cua-driver/scripts" "$FAKE_REPO/libs/lume/scripts" \
    "$FAKE_REPO/libs/cua" "$FAKE_REPO/libs/cua-spacesd" "$FAKE_REPO/libs/spaces-app-swift/scripts" \
    "$SHIMS" "$SHIM_STATE" "$FAKE_HOME"
cp "$SCRIPTS_DIR/install-local.sh" "$SCRIPTS_DIR/uninstall-local.sh" "$SCRIPTS_DIR/_local-common.sh" "$FAKE_SCRIPTS/"
cp "$SIGNING_LIB" "$FAKE_REPO/libs/cua-driver/scripts/"

# A real Mach-O (codesign needs one) that logs how it was called.
cat > "$WORK/stub.c" <<'EOF'
#include <stdio.h>
#include <stdlib.h>
int main(int argc, char **argv) {
    const char *log = getenv("STUB_LOG");
    FILE *f = log ? fopen(log, "a") : NULL;
    if (f) {
        const char *channel = getenv("CUA_CHANNEL");
        fprintf(f, "%s", argv[0]);
        for (int i = 1; i < argc; i++) fprintf(f, " %s", argv[i]);
        fprintf(f, " [CUA_CHANNEL=%s]\n", channel ? channel : "");
        fclose(f);
    }
    return 0;
}
EOF
printf 'int fake_symbol(void) { return 1; }\n' > "$WORK/dylib.c"
# The daemon stubs differ in code, not only in data: the constant is an operand.
printf '#include <stdio.h>\nint main(void) { printf("%%d\\n", DAEMON_VARIANT * 7919); return 0; }\n' > "$WORK/daemon.c"
cc -DDAEMON_VARIANT=1 -o "$DAEMON_CHECKOUT" "$WORK/daemon.c" || { echo "could not compile the checkout daemon stub"; exit 1; }
cc -DDAEMON_VARIANT=2 -o "$DAEMON_PINNED" "$WORK/daemon.c" || { echo "could not compile the pinned daemon stub"; exit 1; }
cc -o "$STUB_BIN" "$WORK/stub.c" || { echo "could not compile the stub"; exit 1; }
cc -dynamiclib -o "$STUB_DYLIB" "$WORK/dylib.c" || { echo "could not compile the stub library"; exit 1; }

# A build-app.sh that writes a Cua Spaces Local bundle shaped like the real one:
# the app, the bundled cua and cua-spacesd, a loose dylib, a versioned framework
# (like Sparkle), the login item helper plist. FAKE_BUILD_* change what it makes.
cat > "$FAKE_SCRIPTS/build-app.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
echo "$*" >> "$FAKE_BUILD_LOG"
[ "$1" = --channel ] && [ "$2" = local ] || { echo "fake build-app.sh: expected --channel local" >&2; exit 2; }
out="$4"
id="${FAKE_BUILD_ID:-com.trycua.spaces.local}"
name="${FAKE_BUILD_APP_NAME:-Cua Spaces Local}"
scheme="${FAKE_BUILD_SCHEME:-cua-local}"
helper="${FAKE_BUILD_HELPER_LABEL:-com.trycua.spaces.local.helper}"
associated="${FAKE_BUILD_ASSOCIATED:-$id}"
app="$out/$name.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$app/Contents/Frameworks" "$app/Contents/Library/LaunchAgents"
for b in CuaSpacesMac cua; do cp "$FAKE_STUB" "$app/Contents/MacOS/$b"; done
# The host service, as the real build-app.sh gets it: from fetch-spacesd.sh,
# which prints $CUA_SPACESD_BIN when set and the pinned release otherwise.
# FAKE_BUILD_IGNORE_SPACESD_BIN stands for a build-app.sh that does not honour it.
if [ "${FAKE_BUILD_IGNORE_SPACESD_BIN:-0}" = 1 ]; then unset CUA_SPACESD_BIN; fi
spacesd="$("$(dirname "$0")/fetch-spacesd.sh")"
cp "$spacesd" "$app/Contents/MacOS/cua-spacesd"
chmod 0755 "$app/Contents/MacOS/cua-spacesd"
cp "$FAKE_DYLIB" "$app/Contents/Frameworks/libfake.dylib"
fw="$app/Contents/Frameworks/Fake.framework"
mkdir -p "$fw/Versions/A/Resources"
cp "$FAKE_DYLIB" "$fw/Versions/A/Fake"
install_name_tool -id @rpath/Fake.framework/Versions/A/Fake "$fw/Versions/A/Fake"
cat > "$fw/Versions/A/Resources/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>Fake</string>
<key>CFBundleIdentifier</key><string>com.example.fake</string>
<key>CFBundlePackageType</key><string>FMWK</string>
<key>CFBundleVersion</key><string>1</string>
</dict></plist>
PLIST
ln -s A "$fw/Versions/Current"
ln -s Versions/Current/Fake "$fw/Fake"
ln -s Versions/Current/Resources "$fw/Resources"
echo "${FAKE_BUILD_SALT:-1}" > "$app/Contents/Resources/salt.txt"
feed=""
if [ "${FAKE_BUILD_FEED:-0}" = 1 ]; then
  feed="<key>SUFeedURL</key><string>https://example.invalid/appcast.xml</string><key>SUPublicEDKey</key><string>key</string>"
fi
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>CuaSpacesMac</string>
<key>CFBundleIdentifier</key><string>$id</string>
<key>CFBundleName</key><string>$name</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.0.0</string>
<key>CFBundleVersion</key><string>1</string>
<key>CFBundleURLTypes</key><array><dict>
  <key>CFBundleURLName</key><string>$id</string>
  <key>CFBundleURLSchemes</key><array><string>$scheme</string></array>
</dict></array>
<key>CuaChannel</key><string>${FAKE_BUILD_CHANNEL:-local}</string>
<key>CuaHelperLabel</key><string>$helper</string>
$feed
</dict></plist>
PLIST
cat > "$app/Contents/Library/LaunchAgents/$helper.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>Label</key><string>$helper</string>
<key>BundleProgram</key><string>Contents/MacOS/CuaSpacesMac</string>
<key>AssociatedBundleIdentifiers</key><array><string>$associated</string></array>
</dict></plist>
PLIST
/usr/bin/codesign -s - -f -i "$id" "$app/Contents/MacOS/cua-spacesd" >/dev/null 2>&1
/usr/bin/codesign -s - -f -i "$id" "$app" >/dev/null 2>&1
echo "$app"
EOF
chmod +x "$FAKE_SCRIPTS/build-app.sh"

# fetch-spacesd.sh's contract: $CUA_SPACESD_BIN when set, else the pinned release.
cat > "$FAKE_SCRIPTS/fetch-spacesd.sh" <<'FETCH_EOF'
#!/usr/bin/env bash
if [ -n "${CUA_SPACESD_BIN:-}" ]; then echo "$CUA_SPACESD_BIN"; else echo "$FAKE_PINNED_DAEMON"; fi
FETCH_EOF
chmod +x "$FAKE_SCRIPTS/fetch-spacesd.sh"
cat > "$FAKE_REPO/libs/spaces-app-swift/scripts/stage-library.sh" <<'STAGE_EOF'
#!/usr/bin/env bash
echo "stage-library" >> "$CARGO_LOG"
STAGE_EOF
chmod +x "$FAKE_REPO/libs/spaces-app-swift/scripts/stage-library.sh"

# lume scripts: one that knows lume-local, one that does not (the old one that
# installs over the Stable lume).
write_lume_scripts() {
    local knows="$1"
    if [ "$knows" = yes ]; then
        cat > "$FAKE_REPO/libs/lume/scripts/install-local.sh" <<'EOF'
#!/bin/bash
# installs lume-local
echo "install-local $*" >> "$LUME_LOG"
mkdir -p "$HOME/.local/bin" && : > "$HOME/.local/bin/lume-local"
EOF
        cat > "$FAKE_REPO/libs/lume/scripts/uninstall-local.sh" <<'EOF'
#!/bin/bash
# removes lume-local; accepts --yes
echo "uninstall-local $*" >> "$LUME_LOG"
rm -f "$HOME/.local/bin/lume-local"
EOF
    else
        cat > "$FAKE_REPO/libs/lume/scripts/install-local.sh" <<'EOF'
#!/bin/bash
echo "OLD install-local $*" >> "$LUME_LOG"
: > "$HOME/.local/bin/lume"
EOF
        rm -f "$FAKE_REPO/libs/lume/scripts/uninstall-local.sh"
    fi
}

# Shims for everything that would reach outside the fake HOME. Each logs its
# arguments to shim.log.
write_shim() { printf '#!/bin/bash\n%s\n' "$2" > "$SHIMS/$1"; chmod +x "$SHIMS/$1"; }
write_shim launchctl 'echo "launchctl $*" >> "$SHIM_LOG"'
write_shim tccutil 'echo "tccutil $*" >> "$SHIM_LOG"'
write_shim open 'echo "open $*" >> "$SHIM_LOG"'
write_shim lsregister 'echo "lsregister $*" >> "$SHIM_LOG"'
# security: a Keychain with one item per service until it is deleted; or the
# real tool, for the signing test's throwaway keychain.
write_shim security 'if [ "${SHIM_SECURITY_PASSTHROUGH:-0}" = 1 ]; then exec /usr/bin/security "$@"; fi
echo "security $*" >> "$SHIM_LOG"
if [ "$1" = delete-generic-password ] && [ "$2" = -s ]; then
  marker="$SHIM_STATE/deleted-$3"
  if [ -e "$marker" ]; then exit 44; fi
  : > "$marker"; exit 0
fi
exit 0'
# codesign: the real one, except that it can be made to fail verifying one
# path, to stall (exit like a killed-by-alarm process, no output) on its first
# signing call only (CODESIGN_STALL_ONCE=1) or on every one (CODESIGN_STALL=1).
write_shim codesign 'if [ -n "${FAIL_VERIFY_PATH:-}" ]; then
  verifying=0; for a in "$@"; do [ "$a" = --verify ] && verifying=1; done
  for a in "$@"; do if [ "$verifying" = 1 ] && [ "$a" = "$FAIL_VERIFY_PATH" ]; then echo "shim: verify failed" >&2; exit 1; fi; done
fi
signing=0; for a in "$@"; do [ "$a" = --sign ] && signing=1; done
if [ "$signing" = 1 ]; then
  if [ "${CODESIGN_STALL:-0}" = 1 ]; then exit 142; fi
  if [ "${CODESIGN_STALL_ONCE:-0}" = 1 ] && [ ! -e "$SHIM_STATE/stalled" ]; then : > "$SHIM_STATE/stalled"; exit 142; fi
fi
exec /usr/bin/codesign "$@"'

# cargo: logs where and how it ran, and makes what the build would make. In
# libs/cua-spacesd it makes this checkout's daemon (FAKE_CARGO_NO_DAEMON=1:
# a build that succeeds and makes nothing).
write_shim cargo 'echo "cargo $* [in ${PWD##*/}]" >> "$CARGO_LOG"
case "$PWD" in
  */libs/cua-spacesd)
    if [ "${FAKE_CARGO_NO_DAEMON:-0}" != 1 ]; then
      mkdir -p target/release && cp "$FAKE_CHECKOUT_DAEMON" target/release/cua-spacesd
    fi ;;
  */libs/cua)
    mkdir -p target/release && : > target/release/cua-spaces-cli && : > target/release/libcua_spaces_ffi.dylib ;;
esac
exit 0'

# --- Running a script ---------------------------------------------------------------

STDIN_FILE=/dev/null
OUT=""
RC=0
# run <command...>: the command with the fake HOME and shims; output in $OUT,
# status in $RC.
run() {
    OUT="$(
        export HOME="$FAKE_HOME" PATH="$SHIMS:$PATH" CUA_LSREGISTER="$SHIMS/lsregister" \
            SHIM_LOG SHIM_STATE STUB_LOG FAKE_BUILD_LOG="$BUILD_LOG" FAKE_STUB="$STUB_BIN" \
            FAKE_DYLIB="$STUB_DYLIB" LUME_LOG CARGO_LOG FAKE_CHECKOUT_DAEMON="$DAEMON_CHECKOUT" FAKE_PINNED_DAEMON="$DAEMON_PINNED"
        unset SUDO_USER CUA_DRIVER_LOCAL_SIGNING_KEYCHAIN CUA_DRIVER_LOCAL_SIGNING_IDENTITY CUA_CHANNEL CUA_HOME CUA_ENV \
            CARGO_TARGET_DIR CUA_SPACESD_BIN
        "$@" < "$STDIN_FILE" 2>&1
    )"
    RC=$?
}
# The whole path: the cargo shim builds the library, the CLI and the daemon.
install() { run "$FAKE_SCRIPTS/install-local.sh" --no-launch "$@"; }
uninstall() { run "$FAKE_SCRIPTS/uninstall-local.sh" "$@"; }

# --- Snapshots ------------------------------------------------------------------------

# snap <path relative to the fake HOME>...: every file and symlink below each
# as "path<TAB>mode<TAB>link target", then "path<TAB>sha256" per file, sorted.
# A path that is not there is "absent".
snap() {
    (
        cd "$FAKE_HOME" || exit 1
        for p in "$@"; do
            if [ ! -e "$p" ] && [ ! -L "$p" ]; then
                printf '%s\tabsent\n' "$p"
                continue
            fi
            find "$p" \( -type f -o -type l \) -print0 | LC_ALL=C sort -z | xargs -0 stat -f $'%N\t%Lp\t%Y'
            find "$p" -type f -print0 | LC_ALL=C sort -z | xargs -0 shasum -a 256 | sed 's/^\([0-9a-f]*\)  \(.*\)$/\2\t\1/'
        done
    ) | sed 's#^\./##'
}
snap_all() { snap . | LC_ALL=C sort; }

STABLE_PATHS=(
    ".cua" ".lume" ".local/bin/cua" ".local/bin/lume"
    "Library/Application Support/com.trycua.spaces.macos"
    "Library/LaunchAgents/com.trycua.spacesd.host.plist"
    "Library/LaunchAgents/com.trycua.lume_daemon.plist"
    "Cua Volume" "Applications/Cua Spaces.app"
)
snap_stable() { snap "${STABLE_PATHS[@]}"; }

# The real Stable app, when this Mac has one: read, never written (a listing
# with sizes and modification times: it changes if anything is written).
snap_real_stable_app() {
    local app="/Applications/Cua Spaces.app"
    [ -d "$app" ] || { echo "absent"; return; }
    find "$app" -type f -print0 2>/dev/null | LC_ALL=C sort -z | xargs -0 stat -f $'%N\t%z\t%m' | shasum -a 256
}
REAL_HOME="$HOME"
REAL_STABLE_BEFORE="$(snap_real_stable_app)"
REAL_LOCAL_CLI_BEFORE="$(ls -l "$HOME/.local/bin/cua-local" 2>&1)"

# A Stable install in the fake HOME, with a Stable app next to where the Local
# one goes.
make_stable_install() {
    local d
    mkdir -p "$FAKE_HOME/.cua/helper" "$FAKE_HOME/.lume/vm1" "$FAKE_HOME/.local/bin" \
        "$FAKE_HOME/Library/Application Support/com.trycua.spaces.macos" \
        "$FAKE_HOME/Library/LaunchAgents" "$FAKE_HOME/Cua Volume" \
        "$FAKE_HOME/Applications/Cua Spaces.app/Contents/MacOS"
    echo '{"session":"stable"}' > "$FAKE_HOME/.cua/credentials.json"
    echo '{"label":"com.trycua.spaces.helper"}' > "$FAKE_HOME/.cua/helper/service.json"
    echo 'port = 3211' > "$FAKE_HOME/.cua/config.toml"
    echo '{"name":"vm1"}' > "$FAKE_HOME/.lume/vm1/config.json"
    cp "$STUB_BIN" "$FAKE_HOME/.local/bin/cua"
    cp "$STUB_BIN" "$FAKE_HOME/.local/bin/lume"
    echo '{}' > "$FAKE_HOME/Library/Application Support/com.trycua.spaces.macos/settings.json"
    echo '<plist/>' > "$FAKE_HOME/Library/LaunchAgents/com.trycua.spacesd.host.plist"
    echo '<plist/>' > "$FAKE_HOME/Library/LaunchAgents/com.trycua.lume_daemon.plist"
    echo 'my files' > "$FAKE_HOME/Cua Volume/note.txt"
    d="$FAKE_HOME/Applications/Cua Spaces.app/Contents"
    cp "$STUB_BIN" "$d/MacOS/CuaSpacesMac"
    printf '<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>com.trycua.spaces.macos</string></dict></plist>\n' > "$d/Info.plist"
}

reset_fixture() {
    rm -rf "$FAKE_HOME" "$FAKE_REPO/apps/cua-spaces-macos/.build" "$SHIM_STATE" \
        "$FAKE_REPO/libs/cua-spacesd/target" "$FAKE_REPO/libs/cua/target"
    mkdir -p "$FAKE_HOME" "$SHIM_STATE"
    : > "$SHIM_LOG"; : > "$STUB_LOG"; : > "$BUILD_LOG"; : > "$LUME_LOG"; : > "$CARGO_LOG"
    STDIN_FILE=/dev/null
    write_lume_scripts yes
    make_stable_install
}

LOCAL_APP="$FAKE_HOME/Applications/Cua Spaces Local.app"
LOCAL_CLI="$FAKE_HOME/.local/bin/cua-local"
installed_id() { /usr/libexec/PlistBuddy -c "Print :CFBundleIdentifier" "$1/Contents/Info.plist" 2>/dev/null; }

# Every added path must be inside the Local app or be `cua-local`; nothing may
# be removed or changed. Compares two snap_all outputs.
only_local_additions() {
    local before="$1" after="$2" removed added bad
    removed="$(comm -23 <(printf '%s\n' "$before") <(printf '%s\n' "$after"))"
    [ -z "$removed" ] || { note "removed or changed: $removed"; return 1; }
    added="$(comm -13 <(printf '%s\n' "$before") <(printf '%s\n' "$after") | cut -f1)"
    bad="$(printf '%s\n' "$added" | grep -v -e '^Applications/Cua Spaces Local\.app/' -e '^\.local/bin/cua-local$' | grep -v '^$' || true)"
    [ -z "$bad" ] || { note "unexpected new paths: $bad"; return 1; }
}

# --- 0. The names are CuaChannel::Local's ---------------------------------------------

echo "== names =="
if [ -f "$CHANNEL_RS" ]; then
    names_ok=1
    while IFS= read -r value; do
        grep -F >/dev/null "\"$value\"" "$CHANNEL_RS" || { fail "scripts/_local-common.sh has \"$value\", which channel.rs does not"; names_ok=0; }
    done < <(
        # shellcheck disable=SC1091
        . "$SCRIPTS_DIR/_local-common.sh"
        printf '%s\n' "$LOCAL_BUNDLE_ID" "$LOCAL_APP_DIR_NAME" "$LOCAL_CLI_NAME" "$LOCAL_URL_SCHEME" \
            "$LOCAL_HOME_DIR_NAME" "$LOCAL_APP_SUPPORT_DIR_NAME" "$LOCAL_HELPER_LABEL" "$LOCAL_HOST_LABEL" \
            "$LOCAL_VOLUME_NAME" "$LOCAL_LUME_BIN_NAME" "$LOCAL_LUME_DIR_NAME" "${LOCAL_KEYCHAIN_SERVICES[@]}" \
            "$STABLE_BUNDLE_ID" "$STABLE_URL_SCHEME" "$STABLE_HELPER_LABEL" "$STABLE_APP_PARENT" \
            "$STABLE_APP_DIR_NAME" "$STABLE_HOME_DIR_NAME" "$STABLE_CLI_NAME" "$STABLE_LUME_DIR_NAME" "$STABLE_VOLUME_NAME"
    )
    [ "$names_ok" = 1 ] && pass "every name in _local-common.sh is in CuaChannel (channel.rs)"
else
    note "SKIP: no $CHANNEL_RS"
fi

# --- 1. First install ------------------------------------------------------------------

echo "== install =="
reset_fixture
STABLE_BEFORE="$(snap_stable)"
ALL_BEFORE="$(snap_all)"
install
check "install-local.sh succeeds (status $RC)" '[ "$RC" = 0 ]'
[ "$RC" = 0 ] || note "$OUT"
check "builds with --channel local debug <out>" 'grep >/dev/null -- "^--channel local debug " "$BUILD_LOG"'
check "the app is ~/Applications/Cua Spaces Local.app with the Local bundle id" '[ "$(installed_id "$LOCAL_APP")" = com.trycua.spaces.local ]'
check "the signature verifies (deep, strict)" '/usr/bin/codesign --verify --deep --strict "$LOCAL_APP" 2>/dev/null'
check "nested code was re-signed with the app's identity (cua-spacesd keeps its identifier)" \
    '/usr/bin/codesign -dv "$LOCAL_APP/Contents/MacOS/cua-spacesd" 2>&1 | grep >/dev/null "^Identifier=com.trycua.spaces.local$"'
check "the nested framework is signed and sealed" '/usr/bin/codesign --verify --strict "$LOCAL_APP/Contents/Frameworks/Fake.framework" 2>/dev/null'
check "cua-local links to the bundled cua" '[ "$(readlink "$LOCAL_CLI")" = "$LOCAL_APP/Contents/MacOS/cua" ]'
check "cua-local runs the bundled cua" 'STUB_LOG="$STUB_LOG" "$LOCAL_CLI" status && grep >/dev/null "cua-local status" "$STUB_LOG"'
check "no staging files are left in the build tree" '[ ! -e "$FAKE_REPO/apps/cua-spaces-macos/.build/local-stage" ]'
check "nothing is opened with --no-launch" '! grep >/dev/null "^open " "$SHIM_LOG"'
check "the app is registered with LaunchServices" 'grep >/dev/null "^lsregister -f $LOCAL_APP\$" "$SHIM_LOG"'
check "Stable files are byte-identical after install" '[ "$(snap_stable)" = "$STABLE_BEFORE" ]'
check "the only new paths are the Local app and cua-local" 'only_local_additions "$ALL_BEFORE" "$(snap_all)"'
check "no home or Local state was created" '[ ! -e "$FAKE_HOME/.cua-local" ]'

# --- 2. Rebuild ---------------------------------------------------------------------------

echo "== rebuild =="
ALL_BEFORE="$(snap_all)"
: > "$SHIM_LOG"
FAKE_BUILD_SALT=2 install
check "a rebuild installs over the Local app (status $RC)" '[ "$RC" = 0 ]'
check "it is the new build" '[ "$(cat "$LOCAL_APP/Contents/Resources/salt.txt")" = 2 ]'
check "no backup is left behind" '! ls "$FAKE_HOME"/Applications/*.install-backup.* >/dev/null 2>&1'
check "an ad hoc cdhash change resets the Local TCC rows, and only those" \
    'grep >/dev/null "^tccutil reset ScreenCapture com.trycua.spaces.local\$" "$SHIM_LOG" && ! grep "^tccutil" "$SHIM_LOG" | grep -v "com.trycua.spaces.local"'
check "Stable files are byte-identical after a rebuild" '[ "$(snap_stable)" = "$STABLE_BEFORE" ]'
check "nothing but the Local app changed" '[ "$(snap_all | grep -v "Cua Spaces Local.app")" = "$(printf "%s\n" "$ALL_BEFORE" | grep -v "Cua Spaces Local.app")" ]'

# --- 3. Launch and --prefix ----------------------------------------------------------------

echo "== launch, prefix =="
: > "$SHIM_LOG"
run "$FAKE_SCRIPTS/install-local.sh"
check "without --no-launch the app is opened by its exact path" 'grep >/dev/null "^open $LOCAL_APP\$" "$SHIM_LOG"'

reset_fixture
ALL_BEFORE="$(snap_all)"
install --prefix "$WORK/prefix"
check "--prefix installs under the prefix (status $RC)" '[ "$RC" = 0 ] && [ "$(installed_id "$WORK/prefix/Applications/Cua Spaces Local.app")" = com.trycua.spaces.local ] && [ -L "$WORK/prefix/.local/bin/cua-local" ]'
check "--prefix leaves the whole HOME untouched" '[ "$(snap_all)" = "$ALL_BEFORE" ]'
install --prefix /
check "a prefix that would put the app in /Applications is refused" '[ "$RC" != 0 ] && printf "%s" "$OUT" | grep >/dev/null "/Applications"'
install --prefix relative/dir
check "a relative prefix is refused" '[ "$RC" != 0 ] && printf "%s" "$OUT" | grep >/dev/null "absolute"'

# --- 3b. The host service ------------------------------------------------------------------

# The machine code of a program, which a signature does not change.
text_hash() { otool -X -s __TEXT __text "$1" 2>/dev/null | shasum -a 256 | cut -d' ' -f1; }

echo "== the host service (cua-spacesd) =="
reset_fixture
install
check "the library and CLI are built and staged" \
    'grep >/dev/null "^cargo build --locked --release -p cua-spaces-ffi -p cua-spaces-cli \[in cua\]$" "$CARGO_LOG" && grep >/dev/null "^stage-library" "$CARGO_LOG"'
check "cua-spacesd is built from this checkout's libs/cua-spacesd" \
    'grep >/dev/null "^cargo build --locked --release -p cua-spacesd --bin cua-spacesd \[in cua-spacesd\]$" "$CARGO_LOG"'
check "(the checkout's and the pinned daemon of this fixture differ in code)" '[ "$(text_hash "$DAEMON_CHECKOUT")" != "$(text_hash "$DAEMON_PINNED")" ]'
check "the bundled cua-spacesd is this checkout's build" '[ "$(text_hash "$LOCAL_APP/Contents/MacOS/cua-spacesd")" = "$(text_hash "$DAEMON_CHECKOUT")" ]'
check "and not the pinned release" '[ "$(text_hash "$LOCAL_APP/Contents/MacOS/cua-spacesd")" != "$(text_hash "$DAEMON_PINNED")" ]'

ALL_BEFORE="$(snap_all)"
FAKE_BUILD_SALT=pinned FAKE_BUILD_IGNORE_SPACESD_BIN=1 install
check "a build-app.sh that bundles the pinned cua-spacesd is refused, and the installed app is unchanged" \
    '[ "$RC" != 0 ] && printf "%s" "$OUT" | grep >/dev/null "not the build of this checkout" && [ "$(snap_all)" = "$ALL_BEFORE" ]'
reset_fixture
FAKE_BUILD_IGNORE_SPACESD_BIN=1 install
check "the same on a first install: nothing is installed" '[ "$RC" != 0 ] && [ ! -e "$LOCAL_APP" ] && [ ! -L "$LOCAL_CLI" ]'

reset_fixture
FAKE_CARGO_NO_DAEMON=1 install
check "a cargo run that leaves no cua-spacesd stops the install before the app is built" \
    '[ "$RC" != 0 ] && printf "%s" "$OUT" | grep >/dev/null "cargo did not produce" && [ ! -s "$BUILD_LOG" ] && [ ! -e "$LOCAL_APP" ]'

reset_fixture
install --skip-rust-build
check "--skip-rust-build with no cua-spacesd build is refused before anything is built" \
    '[ "$RC" != 0 ] && printf "%s" "$OUT" | grep >/dev/null "no cua-spacesd build" && [ ! -s "$BUILD_LOG" ] && [ ! -s "$CARGO_LOG" ]'
install
: > "$CARGO_LOG"
FAKE_BUILD_SALT=skip install --skip-rust-build
check "--skip-rust-build uses the existing build: no cargo, the same daemon bundled (status $RC)" \
    '[ "$RC" = 0 ] && [ ! -s "$CARGO_LOG" ] && [ "$(cat "$LOCAL_APP/Contents/Resources/salt.txt")" = skip ] && [ "$(text_hash "$LOCAL_APP/Contents/MacOS/cua-spacesd")" = "$(text_hash "$DAEMON_CHECKOUT")" ]'
reset_fixture
run env CARGO_TARGET_DIR=relative/target "$FAKE_SCRIPTS/install-local.sh" --no-launch
check "a relative CARGO_TARGET_DIR is refused (the daemon could not be found)" '[ "$RC" != 0 ] && printf "%s" "$OUT" | grep >/dev/null "CARGO_TARGET_DIR must be an absolute path"'

# --- 3c. --prefix is resolved before anything is built ------------------------------------------

echo "== --prefix resolves .., symlinks and case, and refuses what the released app owns =="
reset_fixture
LINKS="$WORK/links"
rm -rf "$LINKS" "$WORK/p-apps" "$WORK/p-bin" "$WORK/p-sys" "$WORK/ok" "$WORK/a"
mkdir -p "$LINKS" "$WORK/p-apps" "$WORK/p-bin/.local" "$WORK/p-sys" "$WORK/ok"
ln -s "$FAKE_HOME/.cua" "$LINKS/to-cua"
ln -s "$LINKS/to-cua" "$LINKS/chain"
ln -s "$LINKS/loop" "$LINKS/loop"
ln -s "$FAKE_HOME/.lume" "$WORK/p-apps/Applications"
ln -s "$FAKE_HOME/.cua" "$WORK/p-bin/.local/bin"
ln -s /Applications "$WORK/p-sys/Applications"
ln -s "$WORK/ok" "$LINKS/to-ok"
ALL_BEFORE="$(snap_all)"
for entry in \
    "$FAKE_HOME/.cua|inside the Stable home" \
    "$FAKE_HOME/.cua/deeper/still|nested in the Stable home" \
    "$FAKE_HOME/.lume|Lume's folder" \
    "$FAKE_HOME/Cua Volume|Cua Volume" \
    "$FAKE_HOME/Library/Application Support/com.trycua.spaces.macos|the Stable app's support folder" \
    "$FAKE_HOME/x/../.cua|a .. into the Stable home" \
    "$FAKE_HOME/.cua/sub/..|a trailing .. in the Stable home" \
    "$FAKE_HOME/.CUA|the Stable home in another case" \
    "$LINKS/to-cua|a symlink to the Stable home" \
    "$LINKS/chain|a chain of symlinks to the Stable home" \
    "$WORK/p-apps|an Applications folder that is a symlink into Lume's folder" \
    "$WORK/p-bin|a .local/bin that is a symlink into the Stable home" \
    "$WORK/p-sys|an Applications folder that is a symlink to /Applications" \
    "/Applications/Cua Spaces.app|inside the Stable app" \
    "/System/Volumes/Data/Applications|/Applications by its firmlink path"; do
    prefix="${entry%%|*}"
    what="${entry#*|}"
    : > "$BUILD_LOG"; : > "$CARGO_LOG"
    install --prefix "$prefix"
    check "install refuses --prefix $what" \
        '[ "$RC" != 0 ] && printf "%s" "$OUT" | grep >/dev/null "refusing" && [ ! -s "$BUILD_LOG" ] && [ ! -s "$CARGO_LOG" ] && [ "$(snap_all)" = "$ALL_BEFORE" ]'
    : > "$SHIM_LOG"
    uninstall --yes --prefix "$prefix"
    check "uninstall refuses --prefix $what" \
        '[ "$RC" != 0 ] && printf "%s" "$OUT" | grep >/dev/null "refusing" && [ ! -s "$SHIM_LOG" ] && [ "$(snap_all)" = "$ALL_BEFORE" ]'
done
unset entry prefix what
install --prefix "$LINKS/loop"
check "a symlink loop in --prefix is refused" '[ "$RC" != 0 ] && printf "%s" "$OUT" | grep >/dev/null "cannot resolve"'
install --prefix "$FAKE_HOME/.cua/.."
check "a .. that leads out of the Stable home to a safe place is accepted: the default HOME (status $RC)" '[ "$RC" = 0 ] && [ -d "$LOCAL_APP" ]'
reset_fixture
install --prefix "$WORK/a/b/../c"
check "a nested --prefix with .. is installed where it resolves ($WORK/a/c)" '[ "$RC" = 0 ] && [ -d "$WORK/a/c/Applications/Cua Spaces Local.app" ] && [ ! -e "$WORK/a/b" ]'
install --prefix "$LINKS/to-ok"
check "a symlinked --prefix is installed where it leads ($WORK/ok)" '[ "$RC" = 0 ] && [ -d "$WORK/ok/Applications/Cua Spaces Local.app" ] && [ -L "$WORK/ok/.local/bin/cua-local" ]'
check "and the link target is the resolved app" '[ "$(readlink "$WORK/ok/.local/bin/cua-local")" = "$WORK/ok/Applications/Cua Spaces Local.app/Contents/MacOS/cua" ]'
uninstall --yes --prefix "$LINKS/to-ok"
check "uninstall with the same symlinked --prefix removes it" '[ "$RC" = 0 ] && [ ! -e "$WORK/ok/Applications/Cua Spaces Local.app" ] && [ ! -L "$WORK/ok/.local/bin/cua-local" ]'

# --- 3d. A Stable folder that is itself a symlink ------------------------------------------------

# ~/.lume may be a link to another volume (/Volumes/VMs): that volume is the
# Stable Lume folder, and a --prefix there must be refused. The same for every
# Stable-owned path, and for the Stable CLI, a link to the real binary.
echo "== a Stable-owned path that is a symlink: its target is Stable's too =="
for owned in ".lume" ".cua" "Cua Volume" "Library/Application Support/com.trycua.spaces.macos"; do
    reset_fixture
    target="$WORK/elsewhere/$(printf '%s' "$owned" | tr ' /' '__')"
    rm -rf "$WORK/elsewhere"
    mkdir -p "$target"
    cp -R "$FAKE_HOME/$owned/." "$target/"
    rm -rf "${FAKE_HOME:?}/$owned"
    ln -s "$target" "$FAKE_HOME/$owned"
    ALL_BEFORE="$(snap_all)"
    TARGET_BEFORE="$(find "$target" | LC_ALL=C sort)"
    for prefix in "$target" "$target/deeper/still" "$target/x/.."; do
        : > "$BUILD_LOG"; : > "$CARGO_LOG"
        install --prefix "$prefix"
        check "install refuses --prefix $prefix: ~/$owned is a symlink to it" \
            '[ "$RC" != 0 ] && printf "%s" "$OUT" | grep >/dev/null "refusing" && [ ! -s "$BUILD_LOG" ] && [ ! -s "$CARGO_LOG" ] && [ "$(snap_all)" = "$ALL_BEFORE" ] && [ "$(find "$target" | LC_ALL=C sort)" = "$TARGET_BEFORE" ]'
        : > "$SHIM_LOG"
        uninstall --yes --prefix "$prefix"
        check "uninstall refuses --prefix $prefix too" \
            '[ "$RC" != 0 ] && printf "%s" "$OUT" | grep >/dev/null "refusing" && [ ! -s "$SHIM_LOG" ] && [ "$(find "$target" | LC_ALL=C sort)" = "$TARGET_BEFORE" ]'
    done
done
unset owned target prefix

reset_fixture
cp "$FAKE_HOME/.local/bin/cua" "$WORK/cua-real"
rm "$FAKE_HOME/.local/bin/cua"
ln -s "$WORK/cua-real" "$FAKE_HOME/.local/bin/cua"
ALL_BEFORE="$(snap_all)"
install
check "a Stable cua that is a symlink does not stop an install (cua-local is another name)" '[ "$RC" = 0 ] && [ -d "$LOCAL_APP" ] && [ "$(readlink "$FAKE_HOME/.local/bin/cua")" = "$WORK/cua-real" ]'
install --prefix "$WORK/cua-real"
check "but the file it links to is Stable's: --prefix there is refused" '[ "$RC" != 0 ] && printf "%s" "$OUT" | grep >/dev/null "refusing"'
reset_fixture
mkdir -p "$WORK/safe-vms"
ln -s "$WORK/safe-vms" "$WORK/safe-link"
install --prefix "$WORK/safe-link"
check "a symlinked --prefix that Stable does not own is still fine (status $RC)" '[ "$RC" = 0 ] && [ -d "$WORK/safe-vms/Applications/Cua Spaces Local.app" ]'

# --- 4. Refusals ------------------------------------------------------------------------------

echo "== refusals =="
reset_fixture
mkdir -p "$FAKE_HOME/Applications"
cp -R "$FAKE_HOME/Applications/Cua Spaces.app" "$LOCAL_APP"
ALL_BEFORE="$(snap_all)"
install
check "a bundle with the Stable id at the Local path is refused" '[ "$RC" != 0 ] && printf "%s" "$OUT" | grep >/dev/null "refusing to touch" && printf "%s" "$OUT" | grep >/dev/null "com.trycua.spaces.macos"'
check "it was refused before anything was built" '[ ! -s "$BUILD_LOG" ]'
check "and nothing changed" '[ "$(snap_all)" = "$ALL_BEFORE" ]'
uninstall --yes
check "uninstall-local.sh refuses the same bundle too" '[ "$RC" != 0 ] && printf "%s" "$OUT" | grep >/dev/null "refusing to touch"'
check "and removes nothing" '[ "$(snap_all)" = "$ALL_BEFORE" ] && [ -z "$(cat "$SHIM_LOG")" ]'

reset_fixture
mkdir -p "$FAKE_HOME/Applications/Cua Spaces Local.app/Contents"
ALL_BEFORE="$(snap_all)"
install
check "a folder with no Info.plist at the Local path is refused" '[ "$RC" != 0 ] && printf "%s" "$OUT" | grep >/dev/null "not an app bundle"'

reset_fixture
ln -s "$FAKE_HOME/Applications/Cua Spaces.app" "$LOCAL_APP"
ALL_BEFORE="$(snap_all)"
install
check "a symlink at the Local path (here to the Stable app) is refused and left alone" '[ "$RC" != 0 ] && [ "$(snap_all)" = "$ALL_BEFORE" ]'

echo "== a bad build is never installed =="
for variant in "FAKE_BUILD_ID=com.trycua.spaces.macos|bundle id" \
    "FAKE_BUILD_FEED=1|update feed" \
    "FAKE_BUILD_SCHEME=cua|URL scheme" \
    "FAKE_BUILD_CHANNEL=stable|CuaChannel" \
    "FAKE_BUILD_HELPER_LABEL=com.trycua.spaces.helper|Stable helper label" \
    "FAKE_BUILD_ASSOCIATED=com.trycua.spaces.macos|associated with the Stable app"; do
    var="${variant%%|*}"
    what="${variant#*|}"
    reset_fixture
    install
    ALL_BEFORE="$(snap_all)"
    eval "$var FAKE_BUILD_SALT=bad install"
    check "$var: refused ($what), and the installed Local app is unchanged" \
        '[ "$RC" != 0 ] && printf "%s" "$OUT" | grep -i >/dev/null "$what" && [ "$(snap_all)" = "$ALL_BEFORE" ]'
done
reset_fixture
ALL_BEFORE="$(snap_all)"
FAKE_BUILD_APP_NAME="Cua Spaces" install
check "a build-app.sh that ignored --channel local (made Cua Spaces.app) is refused" '[ "$RC" != 0 ] && printf "%s" "$OUT" | grep >/dev/null "did not build the Local channel" && [ "$(snap_all)" = "$ALL_BEFORE" ]'

# --- 5. Rollback ---------------------------------------------------------------------------------

echo "== rollback =="
reset_fixture
install
OLD_APP="$(snap "Applications/Cua Spaces Local.app")"
FAKE_BUILD_SALT=3 FAIL_VERIFY_PATH="$LOCAL_APP" install
check "a copy that does not verify fails the install (status $RC)" '[ "$RC" != 0 ] && printf "%s" "$OUT" | grep >/dev/null "restored the previous"'
check "the previous Local app is back, byte for byte" '[ "$(snap "Applications/Cua Spaces Local.app")" = "$OLD_APP" ]'
check "no backup is left behind" '! ls "$FAKE_HOME"/Applications/*.install-backup.* >/dev/null 2>&1'
check "Stable files are byte-identical after the rollback" '[ "$(snap_stable)" = "$STABLE_BEFORE" ]'

# --- 5b. A stalled codesign -----------------------------------------------------------------------

echo "== a codesign that stalls =="
reset_fixture
ALL_BEFORE="$(snap_all)"
CODESIGN_STALL_ONCE=1 install
check "a codesign that stalls once is tried again, and the install succeeds (status $RC)" '[ "$RC" = 0 ] && [ -d "$LOCAL_APP" ] && [ -e "$SHIM_STATE/stalled" ]'
reset_fixture
ALL_BEFORE="$(snap_all)"
CODESIGN_STALL=1 install
check "a codesign that always stalls fails the install with a clear message, and nothing is installed" \
    '[ "$RC" != 0 ] && printf "%s" "$OUT" | grep >/dev/null "did not finish in 20 s" && [ "$(snap_all)" = "$ALL_BEFORE" ]'

# --- 6. Signing --------------------------------------------------------------------------------------

echo "== signing =="
reset_fixture
install --require-stable-signing
check "--require-stable-signing without a certificate stops, and installs nothing" '[ "$RC" != 0 ] && [ ! -e "$LOCAL_APP" ] && printf "%s" "$OUT" | grep >/dev/null "stable signing is required"'

# The stable local certificate itself (shared with Cua Driver's local install)
# needs a code-signing identity that codesign can use without a prompt, from a
# keychain in the real search list (which a fake HOME hides). Only a Mac set up
# for it has one, and a unit test must not create one in a real keychain. So
# these checks are opt-in: point CUA_TEST_SIGNING_KEYCHAIN at the keychain that
# holds (or may get) "CuaDriver Local Signing (cua-driver-rs)", as in
# libs/cua-driver/scripts/README.md "Stable macOS local signing":
#   CUA_TEST_SIGNING_KEYCHAIN=~/Library/Keychains/cua-driver-signing.keychain-db \
#     scripts/tests/install-local-test.sh
# They run as your real user but install under --prefix in the work directory,
# so nothing of yours is written.
if [ -n "${CUA_TEST_SIGNING_KEYCHAIN:-}" ]; then
    reset_fixture
    SIGNED_PREFIX="$WORK/prefix-signed"
    SIGNED_APP="$SIGNED_PREFIX/Applications/Cua Spaces Local.app"
    ALL_BEFORE="$(snap_all)"
    signed_install() {
        run env HOME="$REAL_HOME" CUA_DRIVER_LOCAL_SIGNING_KEYCHAIN="$CUA_TEST_SIGNING_KEYCHAIN" SHIM_SECURITY_PASSTHROUGH=1 \
            "$FAKE_SCRIPTS/install-local.sh" --no-launch --require-stable-signing --prefix "$SIGNED_PREFIX" "$@"
    }
    signed_install
    if [ "$RC" = 0 ]; then
        REQ1="$(/usr/bin/codesign -d -r- "$SIGNED_APP" 2>&1 | sed -n 's/^designated => //p')"
        : > "$SHIM_LOG"
        FAKE_BUILD_SALT=rebuilt signed_install
        REQ2="$(/usr/bin/codesign -d -r- "$SIGNED_APP" 2>&1 | sed -n 's/^designated => //p')"
        check "with the local certificate the designated requirement is the certificate's" 'printf "%s" "$REQ1" | grep >/dev/null "certificate leaf"'
        check "a rebuild keeps the same designated requirement, so TCC grants survive" '[ -n "$REQ1" ] && [ "$REQ1" = "$REQ2" ] && [ "$(cat "$SIGNED_APP/Contents/Resources/salt.txt")" = rebuilt ]'
        check "a rebuild signed with the certificate resets no TCC rows" '! grep >/dev/null "^tccutil" "$SHIM_LOG"'
        check "every nested piece is signed with the same certificate" \
            'for f in Contents/MacOS/cua Contents/MacOS/cua-spacesd Contents/Frameworks/libfake.dylib Contents/Frameworks/Fake.framework; do /usr/bin/codesign -d -r- "$SIGNED_APP/$f" 2>&1 | grep >/dev/null "certificate leaf" || exit 1; done'
        check "the fake HOME was not written" '[ "$(snap_all)" = "$ALL_BEFORE" ]'
    else
        fail "install with the signing keychain $CUA_TEST_SIGNING_KEYCHAIN failed (status $RC)"
        note "$OUT"
    fi
else
    note "SKIP: certificate-signing checks (set CUA_TEST_SIGNING_KEYCHAIN to run them)"
fi

# --- 7. lume-local ----------------------------------------------------------------------------------------

echo "== lume-local =="
reset_fixture
write_lume_scripts no
ALL_BEFORE="$(snap_all)"
install --with-lume-local
check "--with-lume-local refuses a checkout whose Lume script still installs over Stable, before building" \
    '[ "$RC" != 0 ] && printf "%s" "$OUT" | grep >/dev/null "still installs over the Stable lume" && [ ! -s "$BUILD_LOG" ] && [ ! -s "$LUME_LOG" ] && [ "$(snap_all)" = "$ALL_BEFORE" ]'
write_lume_scripts yes
install --with-lume-local --prefix "$WORK/p2"
check "--with-lume-local cannot be combined with --prefix" '[ "$RC" != 0 ] && printf "%s" "$OUT" | grep >/dev/null "cannot be combined"'
install --with-lume-local
check "--with-lume-local runs the Lume script (status $RC)" '[ "$RC" = 0 ] && grep >/dev/null "^install-local" "$LUME_LOG" && [ -e "$FAKE_HOME/.local/bin/lume-local" ]'
check "and Stable files, lume included, are byte-identical" '[ "$(snap_stable)" = "$STABLE_BEFORE" ]'

# --- 8. Uninstall --------------------------------------------------------------------------------------------------

echo "== uninstall =="
reset_fixture
ALL_BEFORE="$(snap_all)"
STABLE_BEFORE="$(snap_stable)"
install
mkdir -p "$FAKE_HOME/.cua-local/helper" "$FAKE_HOME/Library/Application Support/com.trycua.spaces.local" "$FAKE_HOME/Cua Volume Local"
echo '{}' > "$FAKE_HOME/.cua-local/helper/service.json"
echo '{}' > "$FAKE_HOME/.cua-local/credentials.json"
echo '{}' > "$FAKE_HOME/Library/Application Support/com.trycua.spaces.local/settings.json"
echo '<plist/>' > "$FAKE_HOME/Library/LaunchAgents/com.trycua.spacesd.host.local.plist"
echo 'mine' > "$FAKE_HOME/Cua Volume Local/keep.txt"
BEFORE_UNINSTALL="$(snap_all)"
: > "$SHIM_LOG"; : > "$STUB_LOG"

STDIN_FILE="$WORK/answer-n"; echo n > "$STDIN_FILE"
uninstall
check "answering n removes nothing" '[ "$RC" = 0 ] && [ "$(snap_all)" = "$BEFORE_UNINSTALL" ] && printf "%s" "$OUT" | grep >/dev/null cancelled'
STDIN_FILE=/dev/null
uninstall
check "no answer (no terminal) removes nothing" '[ "$(snap_all)" = "$BEFORE_UNINSTALL" ]'

STDIN_FILE="$WORK/answer-y"; echo y > "$STDIN_FILE"
uninstall
check "answering y removes Local (status $RC)" '[ "$RC" = 0 ] && [ ! -e "$LOCAL_APP" ] && [ ! -e "$LOCAL_CLI" ] && [ ! -L "$LOCAL_CLI" ]'
STDIN_FILE=/dev/null
check "the Local home and its app support folder are gone" '[ ! -e "$FAKE_HOME/.cua-local" ] && [ ! -e "$FAKE_HOME/Library/Application Support/com.trycua.spaces.local" ]'
check "the Local host runner job file is gone" '[ ! -e "$FAKE_HOME/Library/LaunchAgents/com.trycua.spacesd.host.local.plist" ]'
check "the helper login item was unregistered by the Local app itself" 'grep >/dev/null -- "--service-helper unregister \[CUA_CHANNEL=local\]" "$STUB_LOG"'
check "launchctl was asked about the two Local labels only" \
    'grep >/dev/null "bootout gui/[0-9]*/com.trycua.spaces.local.helper$" "$SHIM_LOG" && grep >/dev/null "bootout gui/[0-9]*/com.trycua.spacesd.host.local$" "$SHIM_LOG" && [ "$(grep -c "^launchctl" "$SHIM_LOG")" = 2 ]'
check "only the Local Keychain services were deleted (both of them)" \
    'grep >/dev/null "^security delete-generic-password -s ai.cua.session.local\$" "$SHIM_LOG" && grep >/dev/null "^security delete-generic-password -s ai.cua.session.local.staging\$" "$SHIM_LOG" && ! grep "^security" "$SHIM_LOG" | grep -v -e "-s ai.cua.session.local\$" -e "-s ai.cua.session.local.staging\$"'
check "the Local volume folder is kept" '[ "$(cat "$FAKE_HOME/Cua Volume Local/keep.txt")" = mine ]'
check "Stable files are byte-identical after uninstall" '[ "$(snap_stable)" = "$STABLE_BEFORE" ]'

echo "== install, then uninstall, leaves the HOME as it was =="
reset_fixture
ALL_BEFORE="$(snap_all)"
install
uninstall --yes
check "uninstall --yes after install: the files are exactly the Stable fixture's again" '[ "$RC" = 0 ] && [ "$(snap_all)" = "$ALL_BEFORE" ]'
uninstall --yes
check "uninstall with nothing installed succeeds and changes nothing" '[ "$RC" = 0 ] && [ "$(snap_all)" = "$ALL_BEFORE" ]'

echo "== uninstall and lume-local =="
reset_fixture
install --with-lume-local
: > "$LUME_LOG"
uninstall --yes
check "--yes alone keeps lume-local" '[ -e "$FAKE_HOME/.local/bin/lume-local" ] && [ ! -s "$LUME_LOG" ]'
uninstall --yes --with-lume-local
check "--with-lume-local runs the Lume uninstaller without asking again" '[ "$RC" = 0 ] && grep >/dev/null "^uninstall-local --yes$" "$LUME_LOG" && [ ! -e "$FAKE_HOME/.local/bin/lume-local" ]'
check "Stable files are byte-identical after both" '[ "$(snap_stable)" = "$STABLE_BEFORE" ]'
reset_fixture
install --with-lume-local
STDIN_FILE="$WORK/answer-yn"; printf 'y\nn\n' > "$STDIN_FILE"
: > "$LUME_LOG"
uninstall
check "asked, it offers lume-local, and n keeps it" '[ -e "$FAKE_HOME/.local/bin/lume-local" ] && [ ! -s "$LUME_LOG" ] && printf "%s" "$OUT" | grep >/dev/null "lume-local"'
STDIN_FILE=/dev/null

echo "== this Mac =="
check "the real /Applications/Cua Spaces.app was not touched by any of this" '[ "$(snap_real_stable_app)" = "$REAL_STABLE_BEFORE" ]'
check "this Mac's own ~/.local/bin/cua-local was not touched" '[ "$(ls -l "$HOME/.local/bin/cua-local" 2>&1)" = "$REAL_LOCAL_CLI_BEFORE" ]'

echo
if [ "$fails" = 0 ]; then
    echo "all checks passed"
else
    echo "$fails check(s) FAILED"
fi
exit "$fails"
