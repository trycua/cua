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

mkdir -p "$FAKE_SCRIPTS" "$FAKE_REPO/libs/cua-driver/scripts" "$FAKE_REPO/libs/lume/scripts" \
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
for b in CuaSpacesMac cua cua-spacesd; do cp "$FAKE_STUB" "$app/Contents/MacOS/$b"; done
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
# codesign: the real one, except that it can be made to fail verifying one path.
write_shim codesign 'if [ -n "${FAIL_VERIFY_PATH:-}" ]; then
  verifying=0; for a in "$@"; do [ "$a" = --verify ] && verifying=1; done
  for a in "$@"; do if [ "$verifying" = 1 ] && [ "$a" = "$FAIL_VERIFY_PATH" ]; then echo "shim: verify failed" >&2; exit 1; fi; done
fi
exec /usr/bin/codesign "$@"'

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
            FAKE_DYLIB="$STUB_DYLIB" LUME_LOG
        unset SUDO_USER CUA_DRIVER_LOCAL_SIGNING_KEYCHAIN CUA_DRIVER_LOCAL_SIGNING_IDENTITY CUA_CHANNEL CUA_HOME CUA_ENV
        "$@" < "$STDIN_FILE" 2>&1
    )"
    RC=$?
}
install() { run "$FAKE_SCRIPTS/install-local.sh" --skip-rust-build --no-launch "$@"; }
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
    rm -rf "$FAKE_HOME" "$FAKE_REPO/apps/cua-spaces-macos/.build" "$SHIM_STATE"
    mkdir -p "$FAKE_HOME" "$SHIM_STATE"
    : > "$SHIM_LOG"; : > "$STUB_LOG"; : > "$BUILD_LOG"; : > "$LUME_LOG"
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
            "$STABLE_BUNDLE_ID" "$STABLE_URL_SCHEME" "$STABLE_HELPER_LABEL"
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
run "$FAKE_SCRIPTS/install-local.sh" --skip-rust-build
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
            "$FAKE_SCRIPTS/install-local.sh" --skip-rust-build --no-launch --require-stable-signing --prefix "$SIGNED_PREFIX" "$@"
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
