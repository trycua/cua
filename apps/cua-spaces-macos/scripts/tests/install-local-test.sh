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
#   - the web UI is built as the release build does (and the script stops, with
#     a clear message, when it is missing), and the tools the build needs are
#     checked first, in one message;
#   - on a fresh Keychain (a certificate codesign cannot use yet) it says so and
#     sets it up only when asked (--setup-signing), or prints the exact steps;
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
    "$FAKE_REPO/apps/cua-spaces-web/scripts" "$FAKE_REPO/apps/cua-spaces" \
    "$SHIMS" "$SHIM_STATE" "$FAKE_HOME"
cp "$SCRIPTS_DIR/install-local.sh" "$SCRIPTS_DIR/uninstall-local.sh" "$SCRIPTS_DIR/_local-common.sh" "$FAKE_SCRIPTS/"
cp "$SIGNING_LIB" "$FAKE_REPO/libs/cua-driver/scripts/"
# The pins the script reads (the wasm-bindgen version and the Rust toolchain of
# the web UI's app core), from the real files so they cannot drift from it.
cp "$REPO_ROOT/apps/cua-spaces-web/scripts/build-core-wasm.mjs" "$FAKE_REPO/apps/cua-spaces-web/scripts/"
cp "$REPO_ROOT/apps/cua-spaces/rust-toolchain.toml" "$FAKE_REPO/apps/cua-spaces/"
WEB_LOG="$WORK/web.log"

# A real Mach-O (codesign needs one) that logs how it was called.
cat > "$WORK/stub.c" <<'EOF'
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
int main(int argc, char **argv) {
    const char *log = getenv("STUB_LOG");
    // `CuaSpacesMac --service-helper status`: the login item is registered
    // (STUB_HELPER_STATUS=not-registered: it is not).
    if (argc >= 3 && strcmp(argv[1], "--service-helper") == 0 && strcmp(argv[2], "status") == 0) {
        const char *status = getenv("STUB_HELPER_STATUS");
        printf("{\"status\":\"%s\"}\n", status ? status : "enabled");
    }
    FILE *f = log ? fopen(log, "a") : NULL;
    if (f) {
        const char *channel = getenv("CUA_CHANNEL");
        fprintf(f, "%s", argv[0]);
        for (int i = 1; i < argc; i++) fprintf(f, " %s", argv[i]);
        fprintf(f, " [CUA_CHANNEL=%s]", channel ? channel : "");
        // The home and environment, only when the caller pinned them (the
        // uninstaller does for `host remove`; nothing else sets them here).
        if (getenv("CUA_HOME")) fprintf(f, " [CUA_HOME=%s]", getenv("CUA_HOME"));
        if (getenv("CUA_ENV")) fprintf(f, " [CUA_ENV=%s]", getenv("CUA_ENV"));
        fprintf(f, "\n");
        fclose(f);
    }
    // `cua host remove`: STUB_HOST_REMOVE_HANG=N waits N seconds first (a relay
    // that does not answer), STUB_HOST_REMOVE_STATUS=N exits with N (offline,
    // signed out).
    if (argc >= 3 && strcmp(argv[1], "host") == 0 && strcmp(argv[2], "remove") == 0) {
        const char *hang = getenv("STUB_HOST_REMOVE_HANG");
        const char *status = getenv("STUB_HOST_REMOVE_STATUS");
        if (hang) sleep((unsigned)atoi(hang));
        if (status) return atoi(status);
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
id="${FAKE_BUILD_ID:-com.trycua.spaces.macos.local}"
name="${FAKE_BUILD_APP_NAME:-Cua Spaces Local}"
scheme="${FAKE_BUILD_SCHEME:-cua-local}"
helper="${FAKE_BUILD_HELPER_LABEL:-com.trycua.spaces.local.helper}"
associated="${FAKE_BUILD_ASSOCIATED:-$id}"
app="$out/$name.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Helpers" "$app/Contents/Resources" "$app/Contents/Frameworks" "$app/Contents/Library/LaunchAgents"
for b in CuaSpacesMac cua; do cp "$FAKE_STUB" "$app/Contents/MacOS/$b"; done
# The host service, as the real build-app.sh gets it: from fetch-spacesd.sh,
# which prints $CUA_SPACESD_BIN when set and the pinned release otherwise.
# FAKE_BUILD_IGNORE_SPACESD_BIN stands for a build-app.sh that does not honour it.
if [ "${FAKE_BUILD_IGNORE_SPACESD_BIN:-0}" = 1 ]; then unset CUA_SPACESD_BIN; fi
spacesd="$("$(dirname "$0")/fetch-spacesd.sh")"
cp "$spacesd" "$app/Contents/Helpers/cua-spacesd"
chmod 0755 "$app/Contents/Helpers/cua-spacesd"
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
# The web UI, as the real build-app.sh copies it ($CUA_WEBUI_DIST, else
# apps/cua-spaces-web/dist next to the app's folder).
webui="${CUA_WEBUI_DIST:-$(dirname "$0")/../../cua-spaces-web/dist}"
echo "webui=$webui" >> "$FAKE_BUILD_LOG"
if [ "${FAKE_BUILD_NO_WEBUI:-0}" != 1 ] && [ -f "$webui/index.html" ]; then cp -R "$webui" "$app/Contents/Resources/WebUI"; fi
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
# launchd takes AssociatedBundleIdentifiers as an array or as one string.
if [ "${FAKE_BUILD_ASSOCIATED_SCALAR:-0}" = 1 ]; then
  assoc_xml="<key>AssociatedBundleIdentifiers</key><string>$associated</string>"
else
  assoc_xml="<key>AssociatedBundleIdentifiers</key><array><string>$associated</string></array>"
fi
cat > "$app/Contents/Library/LaunchAgents/$helper.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>Label</key><string>$helper</string>
<key>BundleProgram</key><string>Contents/MacOS/CuaSpacesMac</string>
$assoc_xml
</dict></plist>
PLIST
/usr/bin/codesign -s - -f -i "$id" "$app/Contents/Helpers/cua-spacesd" >/dev/null 2>&1
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
# A fresh Keychain (SHIM_FRESH_KEYCHAIN=1): the certificate is there, but it is
# trusted for code signing only after `add-trusted-cert`, and the Keychain is
# in the search list only after `list-keychains -s` (SHIM_KEYCHAIN_LISTED=0).
if [ "${SHIM_FRESH_KEYCHAIN:-0}" = 1 ]; then
  fake=00112233445566778899AABBCCDDEEFF00112233
  case "$1" in
    find-identity)
      verified=0; for a in "$@"; do [ "$a" = -v ] && verified=1; done
      if [ "$verified" = 0 ] || [ -e "$SHIM_STATE/trusted" ]; then echo "  1) $fake \"CuaDriver Local Signing (cua-driver-rs)\""; fi
      echo "     1 valid identities found"; exit 0 ;;
    find-certificate)
      echo "SHA-1 hash: $fake"; echo "-----BEGIN CERTIFICATE-----"; echo "MIIBfake"; echo "-----END CERTIFICATE-----"; exit 0 ;;
    list-keychains)
      if [ "$3" = -s ]; then : > "$SHIM_STATE/listed"; exit 0; fi
      if [ "${SHIM_KEYCHAIN_LISTED:-1}" = 1 ] || [ -e "$SHIM_STATE/listed" ]; then echo "    \"$HOME/Library/Keychains/login.keychain-db\""; fi
      echo "    \"/Library/Keychains/System.keychain\""; exit 0 ;;
    add-trusted-cert)
      if [ "${SHIM_TRUST_FAILS:-0}" = 1 ]; then echo "SecTrustSettingsSetTrustSettings: no user interaction was possible" >&2; exit 1; fi
      : > "$SHIM_STATE/trusted"; exit 0 ;;
    set-key-partition-list)
      # SHIM_PARTITION_FAILS=1: no key matched the filter (the tool says so and changes nothing).
      if [ "${SHIM_PARTITION_FAILS:-0}" = 1 ]; then echo "security: SecItemCopyMatching: The specified item could not be found in the keychain." >&2; exit 1; fi
      : > "$SHIM_STATE/partition"; exit 0 ;;
  esac
fi
exit 0'
# codesign: the real one, except that it can be made to fail verifying one
# path, to stall (exit like a killed-by-alarm process, no output) on its first
# signing call only (CODESIGN_STALL_ONCE=1) or on every one (CODESIGN_STALL=1).
write_shim codesign 'if [ "${SHIM_FRESH_KEYCHAIN:-0}" = 1 ]; then
  # Signing with the fresh Keychain'"'"'s certificate works once it is trusted (and
  # its key opened to codesign, SHIM_NEED_PARTITION=1); then it signs ad hoc, as
  # a unit test has no real certificate.
  args=(); prev=""; fake=0
  for a in "$@"; do
    if [ "$prev" = --sign ] && [ "$a" = 00112233445566778899AABBCCDDEEFF00112233 ]; then fake=1; args+=(-); else args+=("$a"); fi
    prev="$a"
  done
  if [ "$fake" = 1 ]; then
    if [ ! -e "$SHIM_STATE/trusted" ] || { [ "${SHIM_NEED_PARTITION:-0}" = 1 ] && [ ! -e "$SHIM_STATE/partition" ]; }; then
      echo "no identity found" >&2; exit 1
    fi
    exec /usr/bin/codesign "${args[@]}"
  fi
fi
if [ -n "${FAIL_VERIFY_PATH:-}" ]; then
  verifying=0; for a in "$@"; do [ "$a" = --verify ] && verifying=1; done
  for a in "$@"; do if [ "$verifying" = 1 ] && [ "$a" = "$FAIL_VERIFY_PATH" ]; then echo "shim: verify failed" >&2; exit 1; fi; done
fi
signing=0; for a in "$@"; do [ "$a" = --sign ] && signing=1; done
if [ "$signing" = 1 ]; then
  if [ "${CODESIGN_STALL:-0}" = 1 ]; then exit 142; fi
  if [ "${CODESIGN_STALL_ONCE:-0}" = 1 ] && [ ! -e "$SHIM_STATE/stalled" ]; then : > "$SHIM_STATE/stalled"; exit 142; fi
fi
exec /usr/bin/codesign "$@"'

# The tools the build needs (the script checks for them first). Each can be
# made to look missing or wrong: SHIM_NO_XCODE=1, SHIM_NODE_MAJOR=20,
# SHIM_WASM_TARGET= (none), SHIM_WASM_BINDGEN=0.2.1, SHIM_PNPM_BUILD_FAILS=1.
write_shim xcode-select 'if [ "${SHIM_NO_XCODE:-0}" = 1 ]; then exit 2; fi
echo /Library/Developer/CommandLineTools'
write_shim swift 'echo swift'
write_shim cmake 'echo "cmake version 3.30.0"'
write_shim node 'case "$*" in *versions.node*) echo "${SHIM_NODE_MAJOR:-22}" ;; *) echo "v${SHIM_NODE_MAJOR:-22}.1.0" ;; esac'
write_shim rustup 'case "$*" in "target list --installed --toolchain"*) echo "${SHIM_WASM_TARGET-wasm32-unknown-unknown}" ;; esac'
write_shim wasm-bindgen 'echo "wasm-bindgen ${SHIM_WASM_BINDGEN:-0.2.126}"'
# pnpm: logs where and how it ran; `pnpm build` makes dist/index.html, as the
# web UI's build does.
write_shim pnpm 'echo "pnpm $* [in ${PWD##*/}] CI=${CI:-}" >> "$WEB_LOG"
if [ "$1" = build ]; then
  if [ "${SHIM_PNPM_BUILD_FAILS:-0}" = 1 ]; then echo "vite: build failed" >&2; exit 1; fi
  mkdir -p dist && echo "<!doctype html>" > dist/index.html
fi
exit 0'

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
            FAKE_DYLIB="$STUB_DYLIB" LUME_LOG CARGO_LOG WEB_LOG FAKE_CHECKOUT_DAEMON="$DAEMON_CHECKOUT" FAKE_PINNED_DAEMON="$DAEMON_PINNED"
        # CI is set on a CI runner and not on a Mac. The web UI check expects
        # CI=true only where install-local.sh sets it (pnpm install), so drop it.
        unset SUDO_USER CUA_DRIVER_LOCAL_SIGNING_KEYCHAIN CUA_DRIVER_LOCAL_SIGNING_IDENTITY CUA_CHANNEL CUA_HOME CUA_ENV \
            CARGO_TARGET_DIR CUA_SPACESD_BIN CUA_WEBUI_DIST CI
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
    "Library/Preferences/com.trycua.spaces.macos.plist" "Library/Caches/com.trycua.spaces.macos"
    "Library/HTTPStorages/com.trycua.spaces.macos" "Library/WebKit/com.trycua.spaces.macos"
    "Library/Cookies/com.trycua.spaces.macos.binarycookies" "Library/Saved Application State/com.trycua.spaces.macos.savedState"
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
        "$FAKE_HOME/Library/Preferences" "$FAKE_HOME/Library/Caches/com.trycua.spaces.macos" \
        "$FAKE_HOME/Library/HTTPStorages/com.trycua.spaces.macos" "$FAKE_HOME/Library/WebKit/com.trycua.spaces.macos" \
        "$FAKE_HOME/Library/Cookies" "$FAKE_HOME/Library/Saved Application State/com.trycua.spaces.macos.savedState" \
        "$FAKE_HOME/Library/LaunchAgents" "$FAKE_HOME/Cua Volume" \
        "$FAKE_HOME/Applications/Cua Spaces.app/Contents/MacOS"
    echo '{"session":"stable"}' > "$FAKE_HOME/.cua/credentials.json"
    echo '{"label":"com.trycua.spaces.helper"}' > "$FAKE_HOME/.cua/helper/service.json"
    echo 'port = 3211' > "$FAKE_HOME/.cua/config.toml"
    echo '{"name":"vm1"}' > "$FAKE_HOME/.lume/vm1/config.json"
    cp "$STUB_BIN" "$FAKE_HOME/.local/bin/cua"
    cp "$STUB_BIN" "$FAKE_HOME/.local/bin/lume"
    echo '{}' > "$FAKE_HOME/Library/Application Support/com.trycua.spaces.macos/settings.json"
    echo '<plist/>' > "$FAKE_HOME/Library/Preferences/com.trycua.spaces.macos.plist"
    echo 'cache' > "$FAKE_HOME/Library/Caches/com.trycua.spaces.macos/Cache.db"
    echo 'http' > "$FAKE_HOME/Library/HTTPStorages/com.trycua.spaces.macos/httpstorages.sqlite"
    echo 'web' > "$FAKE_HOME/Library/WebKit/com.trycua.spaces.macos/data"
    echo 'cookies' > "$FAKE_HOME/Library/Cookies/com.trycua.spaces.macos.binarycookies"
    echo 'state' > "$FAKE_HOME/Library/Saved Application State/com.trycua.spaces.macos.savedState/windows.plist"
    echo '<plist/>' > "$FAKE_HOME/Library/LaunchAgents/com.trycua.spacesd.host.plist"
    echo '<plist/>' > "$FAKE_HOME/Library/LaunchAgents/com.trycua.lume_daemon.plist"
    echo 'my files' > "$FAKE_HOME/Cua Volume/note.txt"
    d="$FAKE_HOME/Applications/Cua Spaces.app/Contents"
    cp "$STUB_BIN" "$d/MacOS/CuaSpacesMac"
    printf '<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>com.trycua.spaces.macos</string></dict></plist>\n' > "$d/Info.plist"
}

reset_fixture() {
    rm -rf "$FAKE_HOME" "$FAKE_REPO/apps/cua-spaces-macos/.build" "$SHIM_STATE" \
        "$FAKE_REPO/libs/cua-spacesd/target" "$FAKE_REPO/libs/cua/target" "$FAKE_REPO/apps/cua-spaces-web/dist"
    mkdir -p "$FAKE_HOME" "$SHIM_STATE"
    : > "$SHIM_LOG"; : > "$STUB_LOG"; : > "$BUILD_LOG"; : > "$LUME_LOG"; : > "$CARGO_LOG"; : > "$WEB_LOG"
    STDIN_FILE=/dev/null
    write_lume_scripts yes
    make_stable_install
}

LOCAL_APP="$FAKE_HOME/Applications/Cua Spaces Local.app"
LOCAL_CLI="$FAKE_HOME/.local/bin/cua-local"
installed_id() { /usr/libexec/PlistBuddy -c "Print :CFBundleIdentifier" "$1/Contents/Info.plist" 2>/dev/null; }

# The old Cua Spaces Local, as its first builds (bundle id com.trycua.spaces.local,
# before the rename to com.trycua.spaces.macos.local) left it. make_legacy_data is
# what it keeps in ~/Library; make_legacy_local adds the app at the Local path
# (signed, as every build is), the sign-in and the Local volume.
LEGACY_ID=com.trycua.spaces.local
make_legacy_data() {
    mkdir -p "$FAKE_HOME/Library/Application Support/$LEGACY_ID" "$FAKE_HOME/Library/Preferences" \
        "$FAKE_HOME/Library/Caches/$LEGACY_ID" "$FAKE_HOME/Library/HTTPStorages/$LEGACY_ID" "$FAKE_HOME/Library/WebKit/$LEGACY_ID" \
        "$FAKE_HOME/Library/Cookies" "$FAKE_HOME/Library/Saved Application State/$LEGACY_ID.savedState"
    echo '{}' > "$FAKE_HOME/Library/Application Support/$LEGACY_ID/settings.json"
    echo '<plist/>' > "$FAKE_HOME/Library/Preferences/$LEGACY_ID.plist"
    echo 'cache' > "$FAKE_HOME/Library/Caches/$LEGACY_ID/Cache.db"
    echo 'http' > "$FAKE_HOME/Library/HTTPStorages/$LEGACY_ID/httpstorages.sqlite"
    echo 'web' > "$FAKE_HOME/Library/WebKit/$LEGACY_ID/data"
    echo 'cookies' > "$FAKE_HOME/Library/Cookies/$LEGACY_ID.binarycookies"
    echo 'state' > "$FAKE_HOME/Library/Saved Application State/$LEGACY_ID.savedState/windows.plist"
}
legacy_data_gone() {
    [ ! -e "$FAKE_HOME/Library/Application Support/$LEGACY_ID" ] && [ ! -e "$FAKE_HOME/Library/Preferences/$LEGACY_ID.plist" ] \
        && [ ! -e "$FAKE_HOME/Library/Caches/$LEGACY_ID" ] && [ ! -e "$FAKE_HOME/Library/HTTPStorages/$LEGACY_ID" ] \
        && [ ! -e "$FAKE_HOME/Library/WebKit/$LEGACY_ID" ] && [ ! -e "$FAKE_HOME/Library/Cookies/$LEGACY_ID.binarycookies" ] \
        && [ ! -e "$FAKE_HOME/Library/Saved Application State/$LEGACY_ID.savedState" ]
}
make_legacy_local() {
    local d="$LOCAL_APP/Contents"
    mkdir -p "$d/MacOS" "$FAKE_HOME/.cua-local/helper" "$FAKE_HOME/Cua Volume Local"
    cp "$STUB_BIN" "$d/MacOS/CuaSpacesMac"
    cat > "$d/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>CuaSpacesMac</string>
<key>CFBundleIdentifier</key><string>$LEGACY_ID</string>
<key>CFBundleName</key><string>Cua Spaces Local</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>1</string>
<key>CuaChannel</key><string>local</string>
</dict></plist>
PLIST
    /usr/bin/codesign -s - -f -i "$LEGACY_ID" "$LOCAL_APP" >/dev/null 2>&1
    echo '{"session":"local"}' > "$FAKE_HOME/.cua-local/credentials.json"
    echo '{"label":"com.trycua.spaces.local.helper"}' > "$FAKE_HOME/.cua-local/helper/service.json"
    echo 'my local files' > "$FAKE_HOME/Cua Volume Local/note.txt"
    make_legacy_data
}

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
        printf '%s\n' "$LOCAL_BUNDLE_ID" "$LOCAL_LEGACY_BUNDLE_ID" "$LOCAL_APP_DIR_NAME" "$LOCAL_CLI_NAME" "$LOCAL_URL_SCHEME" \
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
check "the app is ~/Applications/Cua Spaces Local.app with the Local bundle id" '[ "$(installed_id "$LOCAL_APP")" = com.trycua.spaces.macos.local ]'
check "the signature verifies (deep, strict)" '/usr/bin/codesign --verify --deep --strict "$LOCAL_APP" 2>/dev/null'
check "nested code was re-signed with the app's identity (cua-spacesd keeps its identifier)" \
    '/usr/bin/codesign -dv "$LOCAL_APP/Contents/Helpers/cua-spacesd" 2>&1 | grep >/dev/null "^Identifier=com.trycua.spaces.macos.local$"'
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
    'grep >/dev/null "^tccutil reset ScreenCapture com.trycua.spaces.macos.local\$" "$SHIM_LOG" && ! grep "^tccutil" "$SHIM_LOG" | grep -v "com.trycua.spaces.macos.local"'
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
check "--prefix installs under the prefix (status $RC)" '[ "$RC" = 0 ] && [ "$(installed_id "$WORK/prefix/Applications/Cua Spaces Local.app")" = com.trycua.spaces.macos.local ] && [ -L "$WORK/prefix/.local/bin/cua-local" ]'
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
check "the bundled cua-spacesd is this checkout's build" '[ "$(text_hash "$LOCAL_APP/Contents/Helpers/cua-spacesd")" = "$(text_hash "$DAEMON_CHECKOUT")" ]'
check "and not the pinned release" '[ "$(text_hash "$LOCAL_APP/Contents/Helpers/cua-spacesd")" != "$(text_hash "$DAEMON_PINNED")" ]'

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
    '[ "$RC" = 0 ] && [ ! -s "$CARGO_LOG" ] && [ "$(cat "$LOCAL_APP/Contents/Resources/salt.txt")" = skip ] && [ "$(text_hash "$LOCAL_APP/Contents/Helpers/cua-spacesd")" = "$(text_hash "$DAEMON_CHECKOUT")" ]'
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

# The Local id is Stable's plus ".local": an id that only starts with Stable's
# (the notch's, com.trycua.spaces.macos.notch) is neither of them.
reset_fixture
mkdir -p "$FAKE_HOME/Applications"
cp -R "$FAKE_HOME/Applications/Cua Spaces.app" "$LOCAL_APP"
sed -i '' 's#com.trycua.spaces.macos</string>#com.trycua.spaces.macos.notch</string>#' "$LOCAL_APP/Contents/Info.plist"
ALL_BEFORE="$(snap_all)"
install
check "a bundle whose id only starts with the Stable id (.macos.notch) is refused, before anything is built" \
    '[ "$RC" != 0 ] && printf "%s" "$OUT" | grep >/dev/null "refusing to touch" && [ ! -s "$BUILD_LOG" ] && [ "$(snap_all)" = "$ALL_BEFORE" ]'
uninstall --yes
check "uninstall-local.sh refuses it too and removes nothing" '[ "$RC" != 0 ] && printf "%s" "$OUT" | grep >/dev/null "refusing to touch" && [ "$(snap_all)" = "$ALL_BEFORE" ] && [ -z "$(cat "$SHIM_LOG")" ]'

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
    "FAKE_BUILD_ASSOCIATED=com.trycua.spaces.macos|associated with the Stable app" \
    "FAKE_BUILD_ASSOCIATED_SCALAR=1 FAKE_BUILD_ASSOCIATED=com.trycua.spaces.macos|associated with the Stable app"; do
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
FAKE_BUILD_ASSOCIATED_SCALAR=1 install
check "a login item associated with the Local id, as one string or as an array, is accepted (status $RC)" '[ "$RC" = 0 ] && [ "$(installed_id "$LOCAL_APP")" = com.trycua.spaces.macos.local ]'
reset_fixture
ALL_BEFORE="$(snap_all)"
FAKE_BUILD_APP_NAME="Cua Spaces" install
check "a build-app.sh that ignored --channel local (made Cua Spaces.app) is refused" '[ "$RC" != 0 ] && printf "%s" "$OUT" | grep >/dev/null "did not build the Local channel" && [ "$(snap_all)" = "$ALL_BEFORE" ]'

# --- 4b. The web UI ----------------------------------------------------------------------------------

echo "== the web UI =="
reset_fixture
install
WEB_DIST="$FAKE_REPO/apps/cua-spaces-web/dist"
check "the web UI is built as the release build does: pnpm install --ignore-workspace --frozen-lockfile, then pnpm build, in apps/cua-spaces-web" \
    '[ "$(cat "$WEB_LOG")" = "pnpm install --ignore-workspace --frozen-lockfile [in cua-spaces-web] CI=true
pnpm build [in cua-spaces-web] CI=" ]'
check "it is built before the Rust build (the quick one fails first)" '[ -s "$WEB_LOG" ] && [ -s "$CARGO_LOG" ]'
check "the build is told where the web UI is, and the installed app has it" \
    'grep >/dev/null "^webui=$WEB_DIST\$" "$BUILD_LOG" && [ -f "$LOCAL_APP/Contents/Resources/WebUI/index.html" ]'
check "the signature covers the web UI (it verifies, deep and strict)" '/usr/bin/codesign --verify --deep --strict "$LOCAL_APP" 2>/dev/null'

reset_fixture
ALL_BEFORE="$(snap_all)"
install --skip-web-build
check "--skip-web-build with no web UI build stops with a clear message, before anything is built" \
    '[ "$RC" != 0 ] && printf "%s" "$OUT" | grep >/dev/null "The new UI is not in this build" && printf "%s" "$OUT" | grep >/dev/null "pnpm install --ignore-workspace" && [ ! -s "$BUILD_LOG" ] && [ ! -s "$CARGO_LOG" ] && [ ! -s "$WEB_LOG" ]'
check "and nothing is installed" '[ ! -e "$LOCAL_APP" ] && [ "$(snap_all)" = "$ALL_BEFORE" ]'
mkdir -p "$WEB_DIST" && echo "<!doctype html>" > "$WEB_DIST/index.html"
install --skip-web-build
check "--skip-web-build with a web UI build already there installs it, and runs no pnpm" \
    '[ "$RC" = 0 ] && [ ! -s "$WEB_LOG" ] && [ -f "$LOCAL_APP/Contents/Resources/WebUI/index.html" ]'

reset_fixture
mkdir -p "$WORK/other-webui" && echo "<!doctype html>" > "$WORK/other-webui/index.html"
run env CUA_WEBUI_DIST="$WORK/other-webui" "$FAKE_SCRIPTS/install-local.sh" --no-launch
check "CUA_WEBUI_DIST is used as it is: no pnpm, and the build gets it" \
    '[ "$RC" = 0 ] && [ ! -s "$WEB_LOG" ] && grep >/dev/null "^webui=$WORK/other-webui\$" "$BUILD_LOG" && [ -f "$LOCAL_APP/Contents/Resources/WebUI/index.html" ]'
reset_fixture
run env CUA_WEBUI_DIST="$WORK/nowhere" "$FAKE_SCRIPTS/install-local.sh" --no-launch
check "a CUA_WEBUI_DIST without an index.html stops with the same message" \
    '[ "$RC" != 0 ] && printf "%s" "$OUT" | grep >/dev/null "no web UI build at $WORK/nowhere/index.html" && [ ! -e "$LOCAL_APP" ]'

reset_fixture
ALL_BEFORE="$(snap_all)"
run env SHIM_PNPM_BUILD_FAILS=1 "$FAKE_SCRIPTS/install-local.sh" --no-launch
check "a failing web UI build stops the install, and nothing is built after it or installed" \
    '[ "$RC" != 0 ] && printf "%s" "$OUT" | grep >/dev/null "the web UI build failed" && [ ! -s "$BUILD_LOG" ] && [ ! -s "$CARGO_LOG" ] && [ ! -e "$LOCAL_APP" ] && [ "$(snap_all)" = "$ALL_BEFORE" ]'

reset_fixture
install
ALL_BEFORE="$(snap_all)"
FAKE_BUILD_NO_WEBUI=1 FAKE_BUILD_SALT=nowebui install
check "an app build that left the web UI out is never installed: the previous Local app stays" \
    '[ "$RC" != 0 ] && printf "%s" "$OUT" | grep >/dev/null "no web UI" && [ "$(snap_all)" = "$ALL_BEFORE" ]'

# --- 4c. The tools the build needs -------------------------------------------------------------------

echo "== prerequisites =="
# A PATH that has the logging shims but none of the build tools, so what the
# script finds is what the test puts back.
MINIMAL_SHIMS="$WORK/shims-minimal"
reset_minimal_shims() {
    rm -rf "$MINIMAL_SHIMS"; mkdir -p "$MINIMAL_SHIMS"
    for f in "$SHIMS"/*; do
        case "$(basename "$f")" in
            cmake|node|pnpm|rustup|wasm-bindgen|cargo) ;;
            *) cp "$f" "$MINIMAL_SHIMS/" ;;
        esac
    done
}
reset_minimal_shims
MINIMAL_PATH="$MINIMAL_SHIMS:/usr/bin:/bin:/usr/sbin:/sbin"
no_tools_install() { run env PATH="$MINIMAL_PATH" "$FAKE_SCRIPTS/install-local.sh" --no-launch "$@"; }

reset_fixture
ALL_BEFORE="$(snap_all)"
no_tools_install
check "with no build tools it stops before building anything (status $RC)" '[ "$RC" != 0 ] && [ ! -s "$BUILD_LOG" ] && [ ! -s "$CARGO_LOG" ] && [ ! -s "$WEB_LOG" ] && [ ! -e "$LOCAL_APP" ] && [ "$(snap_all)" = "$ALL_BEFORE" ]'
for want in "cmake" "brew install cmake" "rustup.rs" "Node 22 or later" "pnpm" "brew install pnpm" \
    "wasm32 Rust target" "rustup toolchain install 1.97.1 --profile minimal --target wasm32-unknown-unknown" \
    "wasm-bindgen-cli 0.2.126" "cargo install wasm-bindgen-cli --version 0.2.126 --locked"; do
    check "the one message says: $want" 'printf "%s" "$OUT" | grep -F >/dev/null -- "$want"'
done
check "it is one message (one error line), and says nothing was built or changed" \
    '[ "$(printf "%s\n" "$OUT" | grep -c "error:")" = 1 ] && printf "%s" "$OUT" | grep >/dev/null "Nothing was built or changed"'

reset_fixture
# Rust and the web UI already built: the only thing left to ask for is Xcode.
mkdir -p "$FAKE_REPO/libs/cua-spacesd/target/release" "$FAKE_REPO/apps/cua-spaces-web/dist"
cp "$DAEMON_CHECKOUT" "$FAKE_REPO/libs/cua-spacesd/target/release/cua-spacesd"
echo "<!doctype html>" > "$FAKE_REPO/apps/cua-spaces-web/dist/index.html"
SHIM_NO_XCODE=1 no_tools_install --skip-rust-build --skip-web-build
check "without the Xcode command line tools it says to run xcode-select --install" '[ "$RC" != 0 ] && printf "%s" "$OUT" | grep -F >/dev/null "xcode-select --install"'
check "--skip-rust-build --skip-web-build ask for nothing else (no cmake, Rust, Node, pnpm or wasm line)" \
    '! printf "%s" "$OUT" | grep -E >/dev/null "cmake|rustup.rs|Node 22|pnpm|wasm"'
no_tools_install --skip-rust-build --skip-web-build
check "and with Xcode in place those flags need none of the other tools (status $RC)" '[ "$RC" = 0 ] && [ -d "$LOCAL_APP" ] && [ ! -s "$CARGO_LOG" ] && [ ! -s "$WEB_LOG" ]'

reset_fixture
run env SHIM_NODE_MAJOR=20 "$FAKE_SCRIPTS/install-local.sh" --no-launch
check "an old Node is named, with what it has" '[ "$RC" != 0 ] && printf "%s" "$OUT" | grep -F >/dev/null "Node 22 or later, this Mac has v20" && [ ! -s "$BUILD_LOG" ]'
reset_fixture
run env SHIM_WASM_TARGET= SHIM_WASM_BINDGEN=0.2.100 "$FAKE_SCRIPTS/install-local.sh" --no-launch
check "a missing wasm32 target and a wrong wasm-bindgen are listed, and only those" \
    '[ "$RC" != 0 ] && printf "%s" "$OUT" | grep -F >/dev/null "wasm32 Rust target" && printf "%s" "$OUT" | grep -F >/dev/null "wasm-bindgen-cli 0.2.126" && ! printf "%s" "$OUT" | grep -E >/dev/null "cmake|pnpm|Node 22"'
reset_fixture
install
check "with every tool in place the install goes ahead (status $RC)" '[ "$RC" = 0 ] && [ -d "$LOCAL_APP" ]'

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

# --- 5a. The old Local id is replaced -------------------------------------------------------------

echo "== the old Local id is replaced =="
reset_fixture
make_legacy_local
STABLE_BEFORE="$(snap_stable)"
KEPT_BEFORE="$(snap .cua-local "Cua Volume Local")"
: > "$SHIM_LOG"; : > "$STUB_LOG"
install
check "installing over an app with the old id succeeds (status $RC)" '[ "$RC" = 0 ]'
[ "$RC" = 0 ] || note "$OUT"
check "the old id is gone: the app at the Local path has the new Local id" '[ "$(installed_id "$LOCAL_APP")" = com.trycua.spaces.macos.local ] && /usr/bin/codesign --verify --deep --strict "$LOCAL_APP" 2>/dev/null'
check "it says so in one plain line" \
    '[ "$(printf "%s\n" "$OUT" | grep -c "found the old Cua Spaces Local")" = 1 ] && printf "%s\n" "$OUT" | grep "found the old" | grep >/dev/null "found the old Cua Spaces Local (bundle id com.trycua.spaces.local): replacing it with the renamed app (com.trycua.spaces.macos.local)"'
check "the old app was asked to unregister its login item, and the new app registers its own (it was enabled)" \
    'grep >/dev/null -- "--service-helper unregister \[CUA_CHANNEL=local\]" "$STUB_LOG" && [ "$(grep -n -- "--service-helper unregister" "$STUB_LOG" | head -1 | cut -d: -f1)" -lt "$(grep -n -- "--service-helper register" "$STUB_LOG" | head -1 | cut -d: -f1)" ]'
check "launchd was asked about the Local helper label only" \
    '[ "$(grep -c "^launchctl" "$SHIM_LOG")" = 1 ] && grep >/dev/null "bootout gui/[0-9]*/com.trycua.spaces.local.helper$" "$SHIM_LOG"'
check "LaunchServices forgot the old registration, then registered the new app" \
    'grep >/dev/null "^lsregister -u $LOCAL_APP\$" "$SHIM_LOG" && grep >/dev/null "^lsregister -f $LOCAL_APP\$" "$SHIM_LOG"'
check "the old id's settings, preferences, caches, web view data, cookies and window state are gone" 'legacy_data_gone'
check "no permission rows were reset (tccutil cannot resolve an app that is gone, so none is attempted)" '! grep >/dev/null "^tccutil" "$SHIM_LOG"'
check "the sign-in, the helper spec and Cua Volume Local are untouched" '[ "$(snap .cua-local "Cua Volume Local")" = "$KEPT_BEFORE" ]'
check "Stable files are byte-identical" '[ "$(snap_stable)" = "$STABLE_BEFORE" ]'
check "no backup is left behind" '! ls "$FAKE_HOME"/Applications/*.install-backup.* >/dev/null 2>&1'
check "cua-local links to the new app's cua" '[ "$(readlink "$LOCAL_CLI")" = "$LOCAL_APP/Contents/MacOS/cua" ]'
: > "$SHIM_LOG"; : > "$STUB_LOG"
install
check "the next install is a plain rebuild: no migration line, nothing about the old id" \
    '[ "$RC" = 0 ] && ! printf "%s" "$OUT" | grep >/dev/null "found the old" && ! grep >/dev/null "service-helper" "$STUB_LOG"'

# An old app whose login item was not registered gets none registered for the new one.
reset_fixture
make_legacy_local
: > "$STUB_LOG"
STUB_HELPER_STATUS=not-registered install
check "an old Local without a registered login item: replaced, and no login item is registered (status $RC)" \
    '[ "$RC" = 0 ] && [ "$(installed_id "$LOCAL_APP")" = com.trycua.spaces.macos.local ] && ! grep >/dev/null -- "--service-helper register" "$STUB_LOG"'

# A build that is refused leaves the old app, its settings and its login item alone.
reset_fixture
make_legacy_local
ALL_BEFORE="$(snap_all)"
: > "$SHIM_LOG"; : > "$STUB_LOG"
FAKE_BUILD_FEED=1 install
check "a bad build over the old Local is refused and nothing of the old Local changed" \
    '[ "$RC" != 0 ] && [ "$(snap_all)" = "$ALL_BEFORE" ] && ! grep >/dev/null "service-helper" "$STUB_LOG" && ! grep >/dev/null "^launchctl\|^tccutil" "$SHIM_LOG"'

# A copy that fails to verify puts the old app back, and its settings were not touched.
reset_fixture
make_legacy_local
: > "$SHIM_LOG"; : > "$STUB_LOG"
FAKE_BUILD_SALT=3 FAIL_VERIFY_PATH="$LOCAL_APP" install
check "a copy that does not verify restores the old app (the old id, not the new one)" \
    '[ "$RC" != 0 ] && printf "%s" "$OUT" | grep >/dev/null "restored the previous" && [ "$(installed_id "$LOCAL_APP")" = "$LEGACY_ID" ] && ! ls "$FAKE_HOME"/Applications/*.install-backup.* >/dev/null 2>&1'
check "and the old app is known to LaunchServices again and its login item is registered again" \
    'grep >/dev/null -- "--service-helper register \[CUA_CHANNEL=local\]" "$STUB_LOG" && [ "$(grep -n "^lsregister -u $LOCAL_APP\$" "$SHIM_LOG" | tail -1 | cut -d: -f1)" -lt "$(grep -n "^lsregister -f $LOCAL_APP\$" "$SHIM_LOG" | tail -1 | cut -d: -f1)" ]'
check "and the old id's settings are still there" '[ -f "$FAKE_HOME/Library/Application Support/$LEGACY_ID/settings.json" ] && [ -f "$FAKE_HOME/Library/Preferences/$LEGACY_ID.plist" ]'

# Old settings without an old app (it was deleted by hand) are not touched by an install:
# only the uninstaller cleans them.
reset_fixture
make_legacy_data
install
check "old-id settings with no old app are left to the uninstaller (status $RC)" '[ "$RC" = 0 ] && ! legacy_data_gone && ! grep >/dev/null "tccutil" "$SHIM_LOG"'

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

# A fresh Keychain: the certificate exists, but codesign cannot use it until it
# is trusted for code signing, its Keychain is in the search list and codesign
# may use its key. The Keychain tools are shims here (a unit test must not
# change trust settings), so what is checked is what the script asks of them
# and what it tells the person, not macOS.
echo "== a fresh Keychain =="
FAKE_KEYCHAIN="$FAKE_HOME/Library/Keychains/login.keychain-db"
fresh_fixture() {
    reset_fixture
    mkdir -p "$FAKE_HOME/Library/Keychains" && : > "$FAKE_KEYCHAIN"
}
fresh_install() { run env SHIM_FRESH_KEYCHAIN=1 "$@" "$FAKE_SCRIPTS/install-local.sh" --no-launch $FRESH_ARGS; }
security_calls() { grep "^security " "$SHIM_LOG" | grep -v "find-identity\|find-certificate"; }

fresh_fixture
FRESH_ARGS=""
fresh_install
check "without consent it changes no trust setting and no search list" '! security_calls | grep >/dev/null -e "add-trusted-cert" -e "list-keychains .*-s " -e "set-key-partition-list"'
check "it says codesign cannot use the certificate yet, and prints the exact steps (trust, sudo variant, key access)" \
    'printf "%s" "$OUT" | grep -F >/dev/null "codesign cannot use the stable local certificate yet" && printf "%s" "$OUT" | grep -F >/dev/null "security add-trusted-cert -r trustRoot -p codeSign -k \"$FAKE_KEYCHAIN\"" && printf "%s" "$OUT" | grep -F >/dev/null "sudo security add-trusted-cert -d -r trustRoot -p codeSign" && printf "%s" "$OUT" | grep -F >/dev/null "security set-key-partition-list -S apple-tool:,apple:,codesign: -s -t private -l \"CuaDriver Local Signing (cua-driver-rs)\" -k"'
check "the steps are printed once" '[ "$(printf "%s\n" "$OUT" | grep -c "one-time setup")" = 1 ]'
check "the install is ad hoc, reported, as before (status $RC)" '[ "$RC" = 0 ] && printf "%s" "$OUT" | grep -F >/dev/null "signed ad hoc"'

fresh_fixture
FRESH_ARGS="--require-stable-signing"
fresh_install
check "--require-stable-signing still stops, installs nothing, and prints the steps once" \
    '[ "$RC" != 0 ] && [ ! -e "$LOCAL_APP" ] && printf "%s" "$OUT" | grep -F >/dev/null "stable signing is required" && [ "$(printf "%s\n" "$OUT" | grep -c "one-time setup")" = 1 ]'

fresh_fixture
FRESH_ARGS="--setup-signing"
fresh_install
check "--setup-signing trusts the certificate for code signing: security add-trusted-cert -r trustRoot -p codeSign -k <keychain> <certificate>" \
    'grep -E >/dev/null "^security add-trusted-cert -r trustRoot -p codeSign -k $FAKE_KEYCHAIN [^ ]+\$" "$SHIM_LOG"'
check "the trust is user-level (no -d; the script never runs sudo)" '! grep >/dev/null "^security add-trusted-cert .* -d " "$SHIM_LOG" && ! grep >/dev/null "^sudo " "$SHIM_LOG"'
check "the Keychain was already in the search list: it is not rewritten" '! grep >/dev/null "^security list-keychains -d user -s" "$SHIM_LOG"'
check "it asks for no key access when codesign works after the trust" '! grep >/dev/null "set-key-partition-list" "$SHIM_LOG"'
check "it tells the person it worked, and the install goes on (status $RC)" '[ "$RC" = 0 ] && printf "%s" "$OUT" | grep -F >/dev/null "codesign can use the stable local certificate"'
check "the certificate the trust was given for is the one that signs: its PEM is the one that was exported" '! ls "${TMPDIR:-/tmp}"/cua-local-signing.* >/dev/null 2>&1'

fresh_fixture
FRESH_ARGS="--setup-signing"
fresh_install SHIM_KEYCHAIN_LISTED=0
check "a Keychain missing from the search list is added to it, before the trust" \
    'grep -n -e "^security list-keychains -d user -s $FAKE_KEYCHAIN" -e "^security add-trusted-cert" "$SHIM_LOG" | sed -n 1p | grep >/dev/null "list-keychains" && grep >/dev/null "^security add-trusted-cert" "$SHIM_LOG"'

fresh_fixture
FRESH_ARGS="--setup-signing"
fresh_install SHIM_TRUST_FAILS=1
check "when macOS does not take the trust, it says so and prints the steps, and the install is ad hoc (status $RC)" \
    '[ "$RC" = 0 ] && printf "%s" "$OUT" | grep -F >/dev/null "macOS did not trust the certificate" && printf "%s" "$OUT" | grep -F >/dev/null "sudo security add-trusted-cert -d" && printf "%s" "$OUT" | grep -F >/dev/null "signed ad hoc"'

fresh_fixture
FRESH_ARGS="--setup-signing"
fresh_install SHIM_NEED_PARTITION=1
check "when codesign still cannot use the key and there is no terminal, it prints the key-access command rather than asking" \
    '! grep >/dev/null "set-key-partition-list" "$SHIM_LOG" && printf "%s" "$OUT" | grep -F >/dev/null "asked at a terminal" && printf "%s" "$OUT" | grep -F >/dev/null "security set-key-partition-list -S apple-tool:,apple:,codesign: -s -t private -l \"CuaDriver Local Signing (cua-driver-rs)\" -k"'

# The key access is opened for the Local certificate's key only. `-s` alone
# changes every signing key in the Keychain (the login Keychain's other keys
# included), so the call carries `-t private -l <the certificate's name>`. The
# password is asked at a terminal, so these run under a pty that answers it.
cat > "$WORK/pty-run.py" <<'EOF'
import os, pty, select, sys
answer, prompt = os.environ.pop("PTY_ANSWER", ""), os.environ.pop("PTY_PROMPT", "")
pid, fd = pty.fork()
if pid == 0:
    os.execvp(sys.argv[1], sys.argv[1:])
out, sent = b"", False
while True:
    try:
        ready, _, _ = select.select([fd], [], [], 60)
        if not ready:
            os.kill(pid, 9)
            break
        data = os.read(fd, 4096)
    except OSError:
        break
    if not data:
        break
    out += data
    if not sent and prompt.encode() in out:
        os.write(fd, (answer + "\n").encode())
        sent = True
_, status = os.waitpid(pid, 0)
sys.stdout.write(out.decode(errors="replace").replace("\r\n", "\n"))
sys.exit(os.waitstatus_to_exitcode(status))
EOF
tty_install() { run env PTY_ANSWER=kc-secret PTY_PROMPT="Password of the Keychain" /usr/bin/python3 -I "$WORK/pty-run.py" env SHIM_FRESH_KEYCHAIN=1 "$@" "$FAKE_SCRIPTS/install-local.sh" --no-launch --setup-signing; }
KEY_CN="CuaDriver Local Signing (cua-driver-rs)"

fresh_fixture
tty_install SHIM_NEED_PARTITION=1
check "at a terminal it asks for the Keychain password and opens the key for the Local certificate only: -s -t private -l <its name>, once" \
    '[ "$(grep -c "^security set-key-partition-list" "$SHIM_LOG")" = 1 ] && grep -F >/dev/null "security set-key-partition-list -S apple-tool:,apple:,codesign: -s -t private -l $KEY_CN -k kc-secret $FAKE_KEYCHAIN" "$SHIM_LOG"'
check "after that codesign works and it says so (status $RC)" '[ "$RC" = 0 ] && printf "%s" "$OUT" | grep -F >/dev/null "codesign can use the stable local certificate"'

fresh_fixture
tty_install SHIM_NEED_PARTITION=1 SHIM_PARTITION_FAILS=1
check "when no key matches the label it does not widen the filter: one call, then the exact steps" \
    '[ "$(grep -c "^security set-key-partition-list" "$SHIM_LOG")" = 1 ] && ! grep -v -e "-l $KEY_CN" "$SHIM_LOG" | grep >/dev/null "set-key-partition-list" && printf "%s" "$OUT" | grep -F >/dev/null "codesign still cannot use the certificate"'

fresh_fixture
tty_install
check "with the key already usable after the trust it asks for no password and calls no set-key-partition-list" '! grep >/dev/null "set-key-partition-list" "$SHIM_LOG" && ! printf "%s" "$OUT" | grep -F >/dev/null "Password of the Keychain"'

# The real `security`, on a throwaway Keychain (not in the search list, no trust
# change): the function the script runs matches the Local certificate's key and
# no other signing key, and changes nothing when there is no such key.
echo "== the key access, on a real throwaway Keychain =="
if command -v openssl >/dev/null 2>&1; then
    KCDIR="$WORK/kc"; mkdir -p "$KCDIR"
    KCFILE="$KCDIR/test.keychain-db"
    /usr/bin/security create-keychain -p kc-secret "$KCFILE" >/dev/null 2>&1 && /usr/bin/security unlock-keychain -p kc-secret "$KCFILE" >/dev/null 2>&1
    add_signing_key() {
        printf '[req]\ndistinguished_name=dn\nx509_extensions=ext\nprompt=no\n[dn]\nCN=%s\n[ext]\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=critical,codeSigning\n' "$1" > "$KCDIR/req.cnf"
        openssl req -x509 -newkey rsa:2048 -keyout "$KCDIR/k.pem" -out "$KCDIR/c.pem" -days 2 -nodes -config "$KCDIR/req.cnf" >/dev/null 2>&1 || return 1
        { openssl pkcs12 -export -legacy -inkey "$KCDIR/k.pem" -in "$KCDIR/c.pem" -out "$KCDIR/id.p12" -passout pass:x -name "$1" >/dev/null 2>&1 \
            || openssl pkcs12 -export -inkey "$KCDIR/k.pem" -in "$KCDIR/c.pem" -out "$KCDIR/id.p12" -passout pass:x -name "$1" >/dev/null 2>&1; } || return 1
        /usr/bin/security import "$KCDIR/id.p12" -k "$KCFILE" -P x -A -T /usr/bin/codesign >/dev/null 2>&1
    }
    if add_signing_key "Somebody Else's Signing Key" && add_signing_key "$KEY_CN"; then
        # The script's own function; `security` is a wrapper that keeps the keys
        # the tool says it matched (it prints each) in a file.
        MATCHED="$WORK/matched.txt"
        key_access() (
            CUA_LOCAL_SIGN_CN="$1"
            security() { /usr/bin/security "$@" > "$MATCHED" 2>&1; }
            eval "$(sed -n '/^open_key_to_codesign()/,/^}/p' "$SCRIPTS_DIR/install-local.sh")"
            open_key_to_codesign "$KCFILE" kc-secret
        )
        key_access "$KEY_CN"
        check "the function opens the Local certificate's key (status $?)" '[ -s "$MATCHED" ] && grep -F >/dev/null "\"$KEY_CN\"" "$MATCHED"'
        check "and that is the only key it matched: not the other signing key in the same Keychain" '! grep -F >/dev/null "Somebody Else" "$MATCHED" && [ "$(grep -c "^class: 0x00000010" "$MATCHED")" = 1 ]'
        key_access "No such certificate"
        check "with no key of that name it fails and matches nothing" '[ $? != 0 ] || ! grep >/dev/null "^class:" "$MATCHED"'
        # The unscoped call it replaced, to show the test would catch it.
        /usr/bin/security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k kc-secret "$KCFILE" > "$MATCHED" 2>&1
        check "(for reference) the unscoped -s call matches both keys, which is what the scoping prevents" '[ "$(grep -c "^class: 0x00000010" "$MATCHED")" = 2 ]'
    else
        note "SKIP: could not make test keys in a throwaway Keychain"
    fi
    /usr/bin/security delete-keychain "$KCFILE" >/dev/null 2>&1 || true
else
    note "SKIP: no openssl for the throwaway Keychain checks"
fi

fresh_fixture
FRESH_ARGS="--setup-signing"
fresh_install CUA_DRIVER_LOCAL_SIGNING_IDENTITY=00112233445566778899AABBCCDDEEFF00112233
check "an identity the person picked is never trusted or changed for them" \
    '! security_calls | grep >/dev/null -e "add-trusted-cert" -e "list-keychains .*-s " -e "set-key-partition-list" && printf "%s" "$OUT" | grep -F >/dev/null "does not change a certificate you picked"'

fresh_fixture
: > "$SHIM_STATE/trusted"
FRESH_ARGS="--setup-signing"
fresh_install
check "a certificate codesign can already use: no warning and no setup" \
    '! printf "%s" "$OUT" | grep -F >/dev/null "codesign cannot use" && ! security_calls | grep >/dev/null -e "add-trusted-cert" -e "set-key-partition-list"'
check "the Stable files are untouched by all of this" '[ "$(snap_stable)" = "$(snap_stable)" ] && [ ! -e "$FAKE_HOME/.cua-local" ]'

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
            'for f in Contents/MacOS/cua Contents/Helpers/cua-spacesd Contents/Frameworks/libfake.dylib Contents/Frameworks/Fake.framework; do /usr/bin/codesign -d -r- "$SIGNED_APP/$f" 2>&1 | grep >/dev/null "certificate leaf" || exit 1; done'
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
mkdir -p "$FAKE_HOME/.cua-local/helper" "$FAKE_HOME/Library/Application Support/com.trycua.spaces.macos.local" "$FAKE_HOME/Cua Volume Local"
echo '{}' > "$FAKE_HOME/.cua-local/helper/service.json"
echo '{}' > "$FAKE_HOME/.cua-local/credentials.json"
# This Mac is registered on the relay: the host setup's config exists.
mkdir -p "$FAKE_HOME/.cua-local/host"
echo '{}' > "$FAKE_HOME/.cua-local/host/config.json"
echo '{}' > "$FAKE_HOME/Library/Application Support/com.trycua.spaces.macos.local/settings.json"
echo '<plist/>' > "$FAKE_HOME/Library/LaunchAgents/com.trycua.spacesd.host.local.plist"
# What macOS keeps for the Local bundle id in ~/Library.
mkdir -p "$FAKE_HOME/Library/Preferences" "$FAKE_HOME/Library/Caches/com.trycua.spaces.macos.local" "$FAKE_HOME/Library/HTTPStorages/com.trycua.spaces.macos.local" \
    "$FAKE_HOME/Library/WebKit/com.trycua.spaces.macos.local" "$FAKE_HOME/Library/Cookies" "$FAKE_HOME/Library/Saved Application State/com.trycua.spaces.macos.local.savedState"
echo '<plist/>' > "$FAKE_HOME/Library/Preferences/com.trycua.spaces.macos.local.plist"
echo 'cache' > "$FAKE_HOME/Library/Caches/com.trycua.spaces.macos.local/Cache.db"
echo 'http' > "$FAKE_HOME/Library/HTTPStorages/com.trycua.spaces.macos.local/httpstorages.sqlite"
echo 'web' > "$FAKE_HOME/Library/WebKit/com.trycua.spaces.macos.local/data"
echo 'cookies' > "$FAKE_HOME/Library/Cookies/com.trycua.spaces.macos.local.binarycookies"
echo 'state' > "$FAKE_HOME/Library/Saved Application State/com.trycua.spaces.macos.local.savedState/windows.plist"
echo 'mine' > "$FAKE_HOME/Cua Volume Local/keep.txt"
BEFORE_UNINSTALL="$(snap_all)"
: > "$SHIM_LOG"; : > "$STUB_LOG"

STDIN_FILE="$WORK/answer-n"; echo n > "$STDIN_FILE"
uninstall
check "answering n removes nothing" '[ "$RC" = 0 ] && [ "$(snap_all)" = "$BEFORE_UNINSTALL" ] && printf "%s" "$OUT" | grep >/dev/null cancelled'
STDIN_FILE=/dev/null
uninstall
check "no answer (no terminal) removes nothing" '[ "$(snap_all)" = "$BEFORE_UNINSTALL" ]'
check "and the relay is not told anything before the answer is yes" '! grep >/dev/null "host remove" "$STUB_LOG"'

STDIN_FILE="$WORK/answer-y"; echo y > "$STDIN_FILE"
uninstall
check "answering y removes Local (status $RC)" '[ "$RC" = 0 ] && [ ! -e "$LOCAL_APP" ] && [ ! -e "$LOCAL_CLI" ] && [ ! -L "$LOCAL_CLI" ]'
STDIN_FILE=/dev/null
check "the Local home and its app support folder are gone" '[ ! -e "$FAKE_HOME/.cua-local" ] && [ ! -e "$FAKE_HOME/Library/Application Support/com.trycua.spaces.macos.local" ]'
check "the Local bundle id's preferences, caches, web view data, cookies and window state are gone" \
    '[ ! -e "$FAKE_HOME/Library/Preferences/com.trycua.spaces.macos.local.plist" ] && [ ! -e "$FAKE_HOME/Library/Caches/com.trycua.spaces.macos.local" ] && [ ! -e "$FAKE_HOME/Library/HTTPStorages/com.trycua.spaces.macos.local" ] && [ ! -e "$FAKE_HOME/Library/WebKit/com.trycua.spaces.macos.local" ] && [ ! -e "$FAKE_HOME/Library/Cookies/com.trycua.spaces.macos.local.binarycookies" ] && [ ! -e "$FAKE_HOME/Library/Saved Application State/com.trycua.spaces.macos.local.savedState" ]'
check "and Stable's preferences, caches, web view data, cookies and window state of the same kinds are still there" \
    '[ -f "$FAKE_HOME/Library/Preferences/com.trycua.spaces.macos.plist" ] && [ -f "$FAKE_HOME/Library/Caches/com.trycua.spaces.macos/Cache.db" ] && [ -f "$FAKE_HOME/Library/WebKit/com.trycua.spaces.macos/data" ] && [ -f "$FAKE_HOME/Library/Cookies/com.trycua.spaces.macos.binarycookies" ] && [ -f "$FAKE_HOME/Library/Saved Application State/com.trycua.spaces.macos.savedState/windows.plist" ]'
check "the Local host runner job file is gone" '[ ! -e "$FAKE_HOME/Library/LaunchAgents/com.trycua.spacesd.host.local.plist" ]'
check "the helper login item was unregistered by the Local app itself" 'grep >/dev/null -- "--service-helper unregister \[CUA_CHANNEL=local\]" "$STUB_LOG"'
check "the relay was told first: cua-local host remove, pinned to the Local channel, home and environment" \
    'grep >/dev/null "^$FAKE_HOME/.local/bin/cua-local host remove --force --uninstalling \[CUA_CHANNEL=local\] \[CUA_HOME=$FAKE_HOME/.cua-local\] \[CUA_ENV=staging\]\$" "$STUB_LOG" && [ "$(grep -c "host remove" "$STUB_LOG")" = 1 ]'
check "and before the app unregistered its login item and was removed" \
    '[ "$(grep -n "host remove" "$STUB_LOG" | head -1 | cut -d: -f1)" -lt "$(grep -n -- "--service-helper unregister" "$STUB_LOG" | head -1 | cut -d: -f1)" ]'
check "it said so in one plain line, with no warning (nothing to skip)" \
    'printf "%s" "$OUT" | grep >/dev/null "this Mac is off the relay" && ! printf "%s" "$OUT" | grep >/dev/null "could not tell the relay"'
check "Stable's cua was never run" '! grep >/dev/null "^$FAKE_HOME/.local/bin/cua " "$STUB_LOG"'
check "launchctl was asked about the two Local labels only" \
    'grep >/dev/null "bootout gui/[0-9]*/com.trycua.spaces.local.helper$" "$SHIM_LOG" && grep >/dev/null "bootout gui/[0-9]*/com.trycua.spacesd.host.local$" "$SHIM_LOG" && [ "$(grep -c "^launchctl" "$SHIM_LOG")" = 2 ]'
check "only the Local Keychain services were deleted (both of them)" \
    'grep >/dev/null "^security delete-generic-password -s ai.cua.session.local\$" "$SHIM_LOG" && grep >/dev/null "^security delete-generic-password -s ai.cua.session.local.staging\$" "$SHIM_LOG" && ! grep "^security" "$SHIM_LOG" | grep -v -e "-s ai.cua.session.local\$" -e "-s ai.cua.session.local.staging\$"'
check "the Local volume folder is kept" '[ "$(cat "$FAKE_HOME/Cua Volume Local/keep.txt")" = mine ]'
check "Stable files are byte-identical after uninstall" '[ "$(snap_stable)" = "$STABLE_BEFORE" ]'

echo "== uninstall and the relay =="
# A helper for a fixture with a Local install and a host config in each of the
# homes the arguments name (staging: ~/.cua-local; prod: ~/.cua-local/envs/prod).
install_with_host() {
    local env_name
    reset_fixture
    install
    for env_name in "$@"; do
        case "$env_name" in
            staging) mkdir -p "$FAKE_HOME/.cua-local/host"; echo '{}' > "$FAKE_HOME/.cua-local/host/config.json" ;;
            *) mkdir -p "$FAKE_HOME/.cua-local/envs/$env_name/host"; echo '{}' > "$FAKE_HOME/.cua-local/envs/$env_name/host/config.json" ;;
        esac
    done
    STABLE_BEFORE="$(snap_stable)"
    : > "$SHIM_LOG"; : > "$STUB_LOG"
}

# Offline or signed out: host remove exits non-zero. One plain line, and the
# uninstall goes on.
install_with_host staging
STUB_HOST_REMOVE_STATUS=1 uninstall --yes
check "a relay that cannot be told: the uninstall still finishes (status $RC)" \
    '[ "$RC" = 0 ] && [ ! -e "$LOCAL_APP" ] && [ ! -e "$LOCAL_CLI" ] && [ ! -L "$LOCAL_CLI" ] && [ ! -e "$FAKE_HOME/.cua-local" ]'
check "and says so in one plain line" \
    '[ "$(printf "%s\n" "$OUT" | grep -c "could not tell the relay this Mac is gone")" = 1 ] && printf "%s" "$OUT" | grep >/dev/null "offline, signed out" && ! printf "%s" "$OUT" | grep >/dev/null "this Mac is off the relay"'
check "Stable files are byte-identical" '[ "$(snap_stable)" = "$STABLE_BEFORE" ]'

# A relay that does not answer: the call is stopped at the limit.
install_with_host staging
start=$SECONDS
CUA_UNINSTALL_HOST_REMOVE_SECS=2 STUB_HOST_REMOVE_HANG=30 uninstall --yes
elapsed=$((SECONDS - start))
check "a relay that never answers is given up on at the limit, not waited for (${elapsed}s, status $RC)" \
    '[ "$RC" = 0 ] && [ "$elapsed" -lt 25 ] && [ ! -e "$LOCAL_APP" ] && [ ! -e "$FAKE_HOME/.cua-local" ]'
check "it says so in one plain line, naming the limit, and prints nothing from the shell about it" \
    '[ "$(printf "%s\n" "$OUT" | grep -c "could not tell the relay this Mac is gone")" = 1 ] && printf "%s" "$OUT" | grep >/dev/null "no answer in 2s" && ! printf "%s" "$OUT" | grep >/dev/null "Terminated"'

# Not set up for access (no host config): nothing to tell the relay.
install_with_host
uninstall --yes
check "no host setup: the relay is not called, and it says so (status $RC)" \
    '[ "$RC" = 0 ] && ! grep >/dev/null "host remove" "$STUB_LOG" && printf "%s" "$OUT" | grep >/dev/null "not set up for access" && [ ! -e "$LOCAL_APP" ]'

# The link is gone but the app is there: the bundled cua is the same program.
install_with_host staging
rm -f "$LOCAL_CLI"
uninstall --yes
check "without the cua-local link, the app's own cua tells the relay (status $RC)" \
    '[ "$RC" = 0 ] && grep >/dev/null "^$LOCAL_APP/Contents/MacOS/cua host remove --force --uninstalling \[CUA_CHANNEL=local\]" "$STUB_LOG" && ! grep >/dev/null "^$FAKE_HOME/.local/bin/cua " "$STUB_LOG"'

# No CLI at all (a half-removed install): skipped, one line, the rest is removed.
reset_fixture
mkdir -p "$FAKE_HOME/.cua-local/host"
echo '{}' > "$FAKE_HOME/.cua-local/host/config.json"
STABLE_BEFORE="$(snap_stable)"
: > "$SHIM_LOG"; : > "$STUB_LOG"
uninstall --yes
check "no cua-local to run: skipped with one line, and the rest is still removed (status $RC)" \
    '[ "$RC" = 0 ] && [ "$(printf "%s\n" "$OUT" | grep -c "skipped taking this Mac off the relay")" = 1 ] && [ ! -e "$FAKE_HOME/.cua-local" ] && [ ! -s "$STUB_LOG" ]'
check "Stable files are byte-identical" '[ "$(snap_stable)" = "$STABLE_BEFORE" ]'

# The production opt-in keeps its host setup in a home of its own; staging's is
# separate. Each is told, with its own home and environment.
install_with_host staging prod
uninstall --yes
check "staging and production homes are each taken off the relay, with their own home and environment (status $RC)" \
    '[ "$RC" = 0 ] && grep >/dev/null "host remove .*\[CUA_HOME=$FAKE_HOME/.cua-local\] \[CUA_ENV=staging\]\$" "$STUB_LOG" && grep >/dev/null "host remove .*\[CUA_HOME=$FAKE_HOME/.cua-local/envs/prod\] \[CUA_ENV=prod\]\$" "$STUB_LOG" && [ "$(grep -c "host remove" "$STUB_LOG")" = 2 ]'
check "Stable files are byte-identical" '[ "$(snap_stable)" = "$STABLE_BEFORE" ]'

echo "== uninstall and the old Local id =="
reset_fixture
make_legacy_local
STABLE_BEFORE="$(snap_stable)"
: > "$SHIM_LOG"; : > "$STUB_LOG"
uninstall --yes
check "uninstall removes an old-id app at the Local path (status $RC)" '[ "$RC" = 0 ] && [ ! -e "$LOCAL_APP" ] && printf "%s" "$OUT" | grep >/dev/null "found the old"'
check "the old app unregistered its login item, and launchd was asked about the two Local labels only" \
    'grep >/dev/null -- "--service-helper unregister \[CUA_CHANNEL=local\]" "$STUB_LOG" && [ "$(grep -c "^launchctl" "$SHIM_LOG")" = 2 ] && grep >/dev/null "bootout gui/[0-9]*/com.trycua.spaces.local.helper$" "$SHIM_LOG" && grep >/dev/null "bootout gui/[0-9]*/com.trycua.spacesd.host.local$" "$SHIM_LOG"'
check "its settings, preferences, caches, web view data, cookies and window state are gone" 'legacy_data_gone'
check "the Local home is gone, Cua Volume Local is kept" '[ ! -e "$FAKE_HOME/.cua-local" ] && [ "$(cat "$FAKE_HOME/Cua Volume Local/note.txt")" = "my local files" ]'
check "Stable files are byte-identical" '[ "$(snap_stable)" = "$STABLE_BEFORE" ]'

# Both ids' leftovers at once, no app of either (nothing is installed).
reset_fixture
make_legacy_data
mkdir -p "$FAKE_HOME/Library/Application Support/com.trycua.spaces.macos.local" "$FAKE_HOME/Library/Caches/com.trycua.spaces.macos.local"
echo '{}' > "$FAKE_HOME/Library/Application Support/com.trycua.spaces.macos.local/settings.json"
echo 'cache' > "$FAKE_HOME/Library/Caches/com.trycua.spaces.macos.local/Cache.db"
STABLE_BEFORE="$(snap_stable)"
: > "$SHIM_LOG"
uninstall --yes
check "uninstall removes the leftovers of both ids when no app is installed (status $RC)" \
    '[ "$RC" = 0 ] && legacy_data_gone && [ ! -e "$FAKE_HOME/Library/Application Support/com.trycua.spaces.macos.local" ] && [ ! -e "$FAKE_HOME/Library/Caches/com.trycua.spaces.macos.local" ]'
check "and Stable files, whose names the new Local id extends, are byte-identical" '[ "$(snap_stable)" = "$STABLE_BEFORE" ]'
check "uninstall never resets permission rows (macOS keeps them for a reinstall)" '! grep >/dev/null "^tccutil" "$SHIM_LOG"'

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
