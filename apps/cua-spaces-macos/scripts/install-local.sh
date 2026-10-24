#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Builds Cua Spaces from this checkout and installs it as "Cua Spaces Local",
# next to the released Cua Spaces: another bundle id, app, CLI, home, launchd
# labels, ports, URL scheme, Keychain items and no update feed. (CUA-1776. Same
# pattern as Cua Driver's libs/cua-driver/scripts/install-local.sh, which
# installs cua-driver-local.)
#
#   scripts/install-local.sh [--release] [--no-launch] [--prefix DIR]
#                            [--with-lume-local] [--skip-rust-build]
#                            [--skip-web-build] [--setup-signing]
#                            [--require-stable-signing]
#
#   --release            build the release configuration (default: debug)
#   --no-launch          do not open the app when it is installed
#   --prefix DIR         install under DIR instead of $HOME: the app in
#                        DIR/Applications, `cua-local` in DIR/.local/bin
#                        (the state, ~/.cua-local, stays in $HOME). DIR is
#                        resolved (`..`, symlinks) and refused when the app or
#                        the link would be, or be inside, anything the released
#                        Cua Spaces owns (/Applications, ~/.cua,
#                        ~/.local/bin/cua, ~/.lume, ~/Cua Volume)
#   --with-lume-local    also install lume-local (libs/lume/scripts/install-local.sh)
#   --skip-rust-build    do not run cargo and stage the library: use the library,
#                        `cua` and cua-spacesd already built
#   --skip-web-build     do not build the web UI (apps/cua-spaces-web): use the
#                        build already there (dist/), or the one $CUA_WEBUI_DIST
#                        names. The script stops when there is none: without it
#                        the app says "The new UI is not in this build".
#   --setup-signing      when the stable local certificate exists but macOS does
#                        not let codesign use it yet (a fresh Keychain), do the
#                        one-time setup without asking first (see "Signing")
#   --require-stable-signing
#                        stop unless the app can be signed with the stable
#                        local certificate (no ad hoc fallback)
#
# Needs (checked first; one message says what to install): the Xcode command
# line tools, cmake, Rust (rustup) and, for the web UI, Node 22 or later, pnpm,
# the wasm32 Rust target and wasm-bindgen-cli (the versions the release build
# uses).
#
# The bundled host service (cua-spacesd) is built from this checkout, not the
# pinned release: the pinned one does not know the Local ports.
#
# What it installs, and nothing else:
#   ~/Applications/Cua Spaces Local.app
#   ~/.local/bin/cua-local -> the app's bundled `cua`
#
# What it never touches: /Applications/Cua Spaces.app, ~/.local/bin/cua, ~/.cua,
# ~/.lume, their launchd jobs, and any bundle at the Local path whose id is not
# the Local one (it refuses). The build is staged, signed and verified first;
# the live copy is swapped only then, and put back if the copy fails to verify.
#
# An old Cua Spaces Local (the bundle id com.trycua.spaces.local, before the
# rename to com.trycua.spaces.macos.local) at the Local path is replaced: its
# login item is unregistered, the app is removed, and once the new app is
# installed its old settings in ~/Library go too (a failed copy puts the old app
# and its login item back). ~/.cua-local (the sign-in) and Cua Volume Local stay.
# macOS asks for the renamed app's permissions once; the old id's permission rows
# stay in macOS's list (macOS cannot clear them for an app that is gone), unused.
#
# Signing: the stable local certificate shared with Cua Driver's local install
# (libs/cua-driver/scripts/_local-signing.sh), so the Screen Recording and
# Accessibility grants of Cua Spaces Local survive a rebuild. Without it the
# bundle is signed ad hoc and the grants reset on every rebuild (reported).
# On a fresh Keychain the certificate is created, but codesign cannot use it
# until it is trusted for code signing, its Keychain is in the search list and
# codesign may use its key. When that is the case the script says so and, once
# you agree (or with --setup-signing), does it: macOS asks for your password
# once to trust the certificate, and the Keychain password is asked here once
# (not stored) if codesign still needs the key, which is opened for that one
# key only (never every signing key in the Keychain). Where it cannot ask (no
# terminal, no screen) it prints the exact commands instead. It never trusts
# anything without that, never changes which certificate is used, and a
# certificate-backed signature is still what it checks for.
#
# Uninstall: scripts/uninstall-local.sh.
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
APP_DIR="$(cd -- "$SCRIPT_DIR/.." && pwd)"
REPO_ROOT="$(cd -- "$APP_DIR/../.." && pwd)"

# shellcheck source=_local-common.sh
# shellcheck source-path=SCRIPTDIR
. "$SCRIPT_DIR/_local-common.sh"

die() { err "$*"; exit 1; }

# --- Arguments ----------------------------------------------------------------

CONFIG=debug
LAUNCH=1
PREFIX=""
WITH_LUME_LOCAL=0
SKIP_RUST_BUILD=0
SKIP_WEB_BUILD=0
SETUP_SIGNING=0
REQUIRE_STABLE_SIGNING=0

usage() { sed -n '/^# Builds Cua Spaces/,/^# Uninstall:/p' "$0" | sed 's/^# \{0,1\}//'; }

while [ "$#" -gt 0 ]; do
    case "$1" in
        --release) CONFIG=release ;;
        --no-launch) LAUNCH=0 ;;
        --prefix)
            [ "$#" -ge 2 ] || die "--prefix needs a directory"
            PREFIX="$2"
            shift
            ;;
        --prefix=*) PREFIX="${1#*=}" ;;
        --with-lume-local) WITH_LUME_LOCAL=1 ;;
        --skip-rust-build) SKIP_RUST_BUILD=1 ;;
        --skip-web-build) SKIP_WEB_BUILD=1 ;;
        --setup-signing) SETUP_SIGNING=1 ;;
        --require-stable-signing) REQUIRE_STABLE_SIGNING=1 ;;
        --help|-h) usage; exit 0 ;;
        *) err "unknown option: $1 (see --help)"; exit 2 ;;
    esac
    shift
done

if [ "$(id -u)" -eq 0 ] || [ -n "${SUDO_USER:-}" ]; then
    die "do not run this script with sudo or as root."
fi
[ "$(uname -s)" = Darwin ] || die "Cua Spaces Local is a macOS app: run this on a Mac."
[ -n "${HOME:-}" ] || die "HOME is not set."

# --- Where it goes -------------------------------------------------------------

if [ -z "$PREFIX" ]; then
    PREFIX="$HOME"
fi
case "$PREFIX" in
    /*) ;;
    *) die "--prefix must be an absolute path (got: $PREFIX)" ;;
esac
PREFIX="${PREFIX%/}"
# Every destination is resolved first (`.`, `..`, symlinks) and the resolved
# paths are the ones written, so nothing in --prefix can hide where they land.
# The links are resolved on the folders the files go into; `cua-local` itself
# may already be a link, which is replaced, never followed.
APP_PARENT="$(canonicalize_path "$PREFIX/Applications")" || die "cannot resolve $PREFIX/Applications (a symlink loop?)"
BIN_DIR="$(canonicalize_path "$PREFIX/.local/bin")" || die "cannot resolve $PREFIX/.local/bin (a symlink loop?)"
APP_DEST="$APP_PARENT/$LOCAL_APP_DIR_NAME"
CLI_LINK="$BIN_DIR/$LOCAL_CLI_NAME"

# The two paths this script writes outside the build tree have the Local names,
# and neither is, or is inside, anything the released Cua Spaces owns
# (/Applications, ~/.cua, ~/.local/bin/cua, ~/.lume, ~/Cua Volume).
[ "$(basename "$APP_DEST")" = "$LOCAL_APP_DIR_NAME" ] || die "internal error: app path $APP_DEST"
[ "$(basename "$CLI_LINK")" = "$LOCAL_CLI_NAME" ] || die "internal error: CLI path $CLI_LINK"
reject_stable_overlap "app" "$APP_DEST" || exit 1
reject_stable_overlap "$LOCAL_CLI_NAME link" "$CLI_LINK" || exit 1
if [ "$WITH_LUME_LOCAL" = 1 ] && [ "$(canonicalize_path "$PREFIX")" != "$(canonicalize_path "$HOME")" ]; then
    die "--with-lume-local installs under \$HOME and cannot be combined with --prefix."
fi

BUILD_SCRIPT="$SCRIPT_DIR/build-app.sh"
SIGNING_LIB="$REPO_ROOT/libs/cua-driver/scripts/_local-signing.sh"
LUME_INSTALL="$REPO_ROOT/libs/lume/scripts/install-local.sh"
BUILD_OUT="$APP_DIR/.build/local-app"
STAGE_ROOT="$APP_DIR/.build/local-stage"
STAGE_APP="$STAGE_ROOT/$LOCAL_APP_DIR_NAME"
# The host service the app bundles. build-app.sh bundles the pinned release
# (libs/cua-spacesd/VERSION) unless $CUA_SPACESD_BIN names another, and the
# pinned one does not know this checkout's Local ports. So this build bundles
# the cua-spacesd of this checkout.
SPACESD_DIR="$REPO_ROOT/libs/cua-spacesd"
# The web UI the app bundles (New UI): apps/cua-spaces-web, built as the
# release build does (cd-cua-spaces.yml "Build the web UI"). build-app.sh reads
# $CUA_WEBUI_DIST, else apps/cua-spaces-web/dist. A $CUA_WEBUI_DIST is a build
# somebody made: it is used as it is, not built.
WEBUI_APP="$REPO_ROOT/apps/cua-spaces-web"
if [ -n "${CUA_WEBUI_DIST:-}" ]; then
    WEBUI_DIST="$CUA_WEBUI_DIST"
    BUILD_WEB=0
else
    WEBUI_DIST="$WEBUI_APP/dist"
    BUILD_WEB=$((1 - SKIP_WEB_BUILD))
fi
WASM_TOOLCHAIN_FILE="$APP_DIR/../cua-spaces/rust-toolchain.toml"
WASM_SCRIPT="$WEBUI_APP/scripts/build-core-wasm.mjs"
case "${CARGO_TARGET_DIR:-/}" in
    /*) ;;
    *) die "CARGO_TARGET_DIR must be an absolute path (got: $CARGO_TARGET_DIR)." ;;
esac
SPACESD_BIN="${CARGO_TARGET_DIR:-$SPACESD_DIR/target}/release/cua-spacesd"

[ -x "$BUILD_SCRIPT" ] || die "missing $BUILD_SCRIPT"
[ -f "$SIGNING_LIB" ] || die "missing $SIGNING_LIB (the stable local signing shared with Cua Driver)"

# The signing helpers (ensure_local_signing_identity, codesign_bounded,
# designated_requirement, ...) are Cua Driver's, shared rather than copied.
# They read $OS and the colour variables.
# shellcheck disable=SC2034 # read by _local-signing.sh
OS="$(uname -s)"
# shellcheck source=../../../libs/cua-driver/scripts/_local-signing.sh
# shellcheck source-path=SCRIPTDIR
. "$SIGNING_LIB"

# --- Preflight: nothing is built or changed when the Local path holds something else --

require_local_bundle_or_absent "$APP_DEST" || exit 1
if [ -d "$CLI_LINK" ] && [ ! -L "$CLI_LINK" ]; then
    die "refusing to replace $CLI_LINK: it is a directory"
fi
if [ "$SKIP_RUST_BUILD" = 1 ] && [ ! -x "$SPACESD_BIN" ]; then
    die "--skip-rust-build: there is no cua-spacesd build at $SPACESD_BIN. Build it ((cd libs/cua-spacesd && cargo build --locked --release -p cua-spacesd --bin cua-spacesd)) or leave out --skip-rust-build."
fi
if [ "$WITH_LUME_LOCAL" = 1 ]; then
    [ -f "$LUME_INSTALL" ] || die "--with-lume-local: missing $LUME_INSTALL"
    # Without lume-local, that script installs over the Stable lume (the same
    # binary, LaunchAgent, port and VMs).
    if ! grep -q "$LOCAL_LUME_BIN_NAME" "$LUME_INSTALL"; then
        die "--with-lume-local: $LUME_INSTALL in this checkout still installs over the Stable lume (it does not know lume-local). Update to a checkout where lume-local has landed."
    fi
fi

# --- Preflight: the tools the build needs ---------------------------------------------
# Everything that is missing is listed once, with what to run, before anything
# is built or changed.

have() { command -v "$1" >/dev/null 2>&1; }

MISSING=()
missing() { MISSING+=("$1"); }

check_prerequisites() {
    local toolchain wasm_bindgen_version major

    # A shell that did not source ~/.cargo/env still has rustup and cargo here.
    if ! have cargo && [ -f "$HOME/.cargo/env" ]; then
        # shellcheck disable=SC1091
        . "$HOME/.cargo/env"
    fi
    case ":$PATH:" in
        *":$HOME/.cargo/bin:"*) ;;
        *) [ ! -d "$HOME/.cargo/bin" ] || PATH="$HOME/.cargo/bin:$PATH" ;;
    esac

    if ! xcode-select -p >/dev/null 2>&1 || ! have swift; then
        missing "the Xcode command line tools (swift, clang): xcode-select --install"
    fi
    if [ "$SKIP_RUST_BUILD" = 0 ]; then
        have cmake || missing "cmake (the audio codec build needs it): brew install cmake"
    fi
    if [ "$SKIP_RUST_BUILD" = 0 ] || [ "$BUILD_WEB" = 1 ]; then
        have cargo || missing "Rust (cargo and rustup): curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   (then open a new terminal)"
    fi

    [ "$BUILD_WEB" = 1 ] || return 0
    if ! have node; then
        missing "Node 22 or later (the web UI): brew install node"
    else
        major="$(node -p 'process.versions.node.split(".")[0]' 2>/dev/null || echo 0)"
        [ "$major" -ge 22 ] 2>/dev/null || missing "Node 22 or later, this Mac has $(node --version 2>/dev/null || echo none) (the web UI): brew install node"
    fi
    have pnpm || missing "pnpm (the web UI): brew install pnpm   (or: corepack enable)"
    # The web UI's app core is Rust built for the browser (WebAssembly), with
    # the toolchain and the wasm-bindgen the release build pins.
    toolchain="$(sed -n 's/^channel *= *"\(.*\)"/\1/p' "$WASM_TOOLCHAIN_FILE" 2>/dev/null | head -n 1)"
    wasm_bindgen_version="$(sed -n 's/^const WASM_BINDGEN = "\(.*\)"/\1/p' "$WASM_SCRIPT" 2>/dev/null | head -n 1)"
    if [ -n "$toolchain" ]; then
        { have rustup && rustup target list --installed --toolchain "$toolchain" 2>/dev/null | grep -qx wasm32-unknown-unknown; } \
            || missing "the wasm32 Rust target for Rust $toolchain (the web UI; after Rust is installed): rustup toolchain install $toolchain --profile minimal --target wasm32-unknown-unknown"
    fi
    if [ -n "$wasm_bindgen_version" ]; then
        { have wasm-bindgen && wasm-bindgen --version 2>/dev/null | grep -qF "$wasm_bindgen_version"; } \
            || missing "wasm-bindgen-cli $wasm_bindgen_version (the web UI): cargo install wasm-bindgen-cli --version $wasm_bindgen_version --locked"
    fi
}

check_prerequisites
if [ "${#MISSING[@]}" -gt 0 ]; then
    err "Cua Spaces Local cannot be built yet: ${#MISSING[@]} thing(s) to install first. Nothing was built or changed."
    for item in "${MISSING[@]}"; do printf '  - %s\n' "$item" >&2; done
    have brew || echo "Homebrew (https://brew.sh) installs the brew ones." >&2
    echo "Install them, then run this script again." >&2
    exit 1
fi

# The web UI the app bundles: built below, or one that is already there. The
# app without it says "The new UI is not in this build".
web_ui_missing() {
    err "there is no web UI build at $WEBUI_DIST/index.html, and without it Cua Spaces Local shows \"The new UI is not in this build\"."
    err "Build it: (cd apps/cua-spaces-web && pnpm install --ignore-workspace --frozen-lockfile && pnpm build), or leave out --skip-web-build. Nothing was installed."
}
if [ "$BUILD_WEB" = 1 ]; then
    [ -d "$WEBUI_APP" ] || die "missing $WEBUI_APP (the web UI)"
elif [ ! -f "$WEBUI_DIST/index.html" ]; then
    web_ui_missing
    exit 1
fi

printf '%sCua Spaces Local installer%s\n' "$BOLD$BLUE" "$NORMAL"
echo "  source:  $REPO_ROOT"
echo "  config:  $CONFIG"
echo "  app:     $APP_DEST"
echo "  cli:     $CLI_LINK"
echo ""

# --- Build -----------------------------------------------------------------------

# The web UI first (as the release build does: pnpm install --frozen-lockfile,
# pnpm build, which builds the app core's WebAssembly and then the pages): it
# is the quick one to fail. CI=true keeps pnpm from asking questions.
if [ "$BUILD_WEB" = 1 ]; then
    log "building the web UI (apps/cua-spaces-web: pnpm install, pnpm build)"
    if ! (cd "$WEBUI_APP" && CI=true pnpm install --ignore-workspace --frozen-lockfile && pnpm build); then
        die "the web UI build failed (see above). Nothing was installed."
    fi
fi
if [ ! -f "$WEBUI_DIST/index.html" ]; then
    web_ui_missing
    exit 1
fi

if [ "$SKIP_RUST_BUILD" = 0 ]; then
    if ! command -v cargo >/dev/null 2>&1 && [ -f "$HOME/.cargo/env" ]; then
        # shellcheck disable=SC1091
        . "$HOME/.cargo/env"
    fi
    command -v cargo >/dev/null 2>&1 || die "cargo is required (https://rustup.rs), or build the libraries yourself and pass --skip-rust-build."
    log "building the Cua Spaces library and CLI (cargo, release)"
    # The SDK dylib the app links, and the `cua` that goes into the bundle
    # (build-app.sh's defaults).
    (cd "$REPO_ROOT/libs/cua" && cargo build --locked --release -p cua-spaces-ffi -p cua-spaces-cli)
    "$REPO_ROOT/libs/spaces-app-swift/scripts/stage-library.sh"
    log "building cua-spacesd from this checkout (cargo, release)"
    (cd "$SPACESD_DIR" && cargo build --locked --release -p cua-spacesd --bin cua-spacesd)
fi
[ -x "$SPACESD_BIN" ] || die "cargo did not produce $SPACESD_BIN"

# What the bundled daemon must be: this one. Compared by its machine code (the
# __text section), which the signature build-app.sh adds does not change.
code_hash() {
    local hash
    hash="$(otool -X -s __TEXT __text "$1" 2>/dev/null | shasum -a 256 | cut -d' ' -f1)"
    # The hash of nothing: otool found no code.
    [ -n "$hash" ] && [ "$hash" != e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 ] || return 1
    printf '%s' "$hash"
}
SPACESD_CODE_HASH="$(code_hash "$SPACESD_BIN")" || die "cannot read the machine code of $SPACESD_BIN (otool)"

log "building the app (build-app.sh --channel local $CONFIG)"
mkdir -p "$BUILD_OUT"
# build-app.sh prints the bundle path as its last line.
if ! BUILT_APP="$(CUA_SPACESD_BIN="$SPACESD_BIN" CUA_WEBUI_DIST="$WEBUI_DIST" "$BUILD_SCRIPT" --channel local "$CONFIG" "$BUILD_OUT" | tee /dev/stderr | tail -n 1)"; then
    die "build-app.sh failed"
fi
[ -d "$BUILT_APP" ] || BUILT_APP="$BUILD_OUT/$LOCAL_APP_DIR_NAME"
[ -d "$BUILT_APP" ] || die "build-app.sh did not produce an app bundle in $BUILD_OUT"
if [ "$(basename "$BUILT_APP")" != "$LOCAL_APP_DIR_NAME" ]; then
    die "build-app.sh --channel local produced '$(basename "$BUILT_APP")', not '$LOCAL_APP_DIR_NAME': it did not build the Local channel, so nothing was installed."
fi

# --- Stage, sign, verify ---------------------------------------------------------

cleanup() { rm -rf "$STAGE_ROOT"; }
trap cleanup EXIT

rm -rf "$STAGE_ROOT"
mkdir -p "$STAGE_ROOT"
# No extended attributes, resource forks or quarantine flag: codesign refuses
# "resource fork, Finder information, or similar detritus" in a bundle, and a
# copy of a downloaded or Finder-touched app has them.
ditto --noextattr --noqtn "$BUILT_APP" "$STAGE_APP"
xattr -cr "$STAGE_APP" 2>/dev/null || true

# Mach-O file or not: the first four bytes.
is_macho() {
    case "$(head -c 4 "$1" 2>/dev/null | od -An -tx1 | tr -d ' \n')" in
        cffaedfe|cefaedfe|feedfacf|feedface|cafebabe|bebafeca|cafebabf|bfbafeca) return 0 ;;
        *) return 1 ;;
    esac
}

sort_deepest_first() { awk -F/ '{ print NF "\t" $0 }' | sort -rn | cut -f2-; }

# The bundle's code that is not its main executable: loose Mach-O files in the
# places code lives (the bundled `cua`, `cua-spacesd`, the SDK dylib, Sparkle's
# helpers), deepest first. Resources are data and stay sealed, not signed.
nested_code_files() {
    local app="$1" main dir f
    main="$app/Contents/MacOS/$(plist_get "$app/Contents/Info.plist" CFBundleExecutable || true)"
    for dir in MacOS Frameworks PlugIns XPCServices Library Helpers; do
        [ -d "$app/Contents/$dir" ] || continue
        find "$app/Contents/$dir" -type f
    done | while IFS= read -r f; do
        [ "$f" != "$main" ] || continue
        if is_macho "$f"; then printf '%s\n' "$f"; fi
    done | sort_deepest_first
}

# Nested bundles (Sparkle.framework, its XPC services and Updater.app), deepest first.
nested_bundles() {
    find "$1/Contents" -type d \( -name '*.app' -o -name '*.xpc' -o -name '*.framework' \) | sort_deepest_first
}

# One codesign call, with its output kept for the failure message. Each piece
# keeps the identifier (and entitlements) build-app.sh gave it, such as
# com.trycua.cua-env-driver for cua-spacesd; only the identity changes. The
# call is bounded (a Keychain prompt nobody answers would wait forever), and a
# Mac that stalls for that long (a full disk, a busy trustd) gets a second try.
sign_piece() {
    local identity="$1" target="$2" preserve="$3" out="" status=0
    local args=(--force --sign "$identity")
    [ -z "$preserve" ] || args+=("--preserve-metadata=$preserve")
    for _ in 1 2; do
        out="$(codesign_bounded 20 "${args[@]}" "$target" 2>&1)" && return 0
        status=$?
    done
    err "codesign failed for $target"
    case "$status" in
        124|137|142) err "it did not finish in 20 s, twice (a Keychain prompt waiting for you, or a Mac too busy or too full to sign)" ;;
    esac
    [ -z "$out" ] || printf '%s\n' "$out" >&2
    return 1
}

# Signs the staged app with `identity`, inside out: the nested code, the nested
# bundles, then the app (whose identifier is its bundle id). Not `--deep`: that
# would give everything inside the app's options and a derived identifier.
sign_bundle_with() {
    local identity="$1" app="$2" f
    while IFS= read -r f; do
        [ -z "$f" ] || sign_piece "$identity" "$f" identifier,entitlements || return 1
    done < <(nested_code_files "$app")
    while IFS= read -r f; do
        [ -z "$f" ] || sign_piece "$identity" "$f" identifier,entitlements || return 1
    done < <(nested_bundles "$app")
    sign_piece "$identity" "$app" "" || return 1
    codesign --verify --deep --strict "$app" 2>/dev/null
}

# Lets codesign use the private key of the stable local certificate without a
# prompt. `-l` limits it to the key labelled with the certificate's name
# ($CUA_LOCAL_SIGN_CN; the label the helper gives it when it imports the
# identity), and `-t private` to private keys: without them `-s` would change
# the access rules of every signing key in the Keychain, the login Keychain's
# other keys included. When no key has that label the tool fails and changes
# nothing. Prints nothing and takes the Keychain password as $2.
open_key_to_codesign() {
    security set-key-partition-list -S apple-tool:,apple:,codesign: -s -t private -l "$CUA_LOCAL_SIGN_CN" -k "$2" "$1" >/dev/null 2>&1
}

# Whether `keychain` is in the user's Keychain search list.
keychain_listed() {
    security list-keychains -d user 2>/dev/null | tr -d ' "' | grep -qxF "$1"
}

# Whether the Keychain holds `identity` as a code-signing identity macOS
# trusts (the `-v` list leaves out one that is not trusted yet).
identity_trusted() {
    security find-identity -v -p codesigning "$2" 2>/dev/null | grep -qF "$1"
}

# The one-time setup, as the commands to type, for a Mac where the script
# could not (or was not allowed to) do it.
SIGNING_HELP_SHOWN=0
print_signing_bootstrap() {
    local kc pem="/tmp/cua-local-signing.pem"
    # Once per run, whichever step needed to say it first.
    [ "$SIGNING_HELP_SHOWN" = 0 ] || return 0
    SIGNING_HELP_SHOWN=1
    kc="$(local_signing_keychain)"
    echo "Cua Spaces Local is signed with the stable local certificate \"$CUA_LOCAL_SIGN_CN\"" >&2
    echo "(the one Cua Driver's local install uses), so its Screen Recording and" >&2
    echo "Accessibility permissions survive rebuilds. On a new Keychain that needs a" >&2
    echo "one-time setup, in Terminal on this Mac:" >&2
    echo "  security find-certificate -c \"$CUA_LOCAL_SIGN_CN\" -p \"$kc\" > $pem" >&2
    echo "  security add-trusted-cert -r trustRoot -p codeSign -k \"$kc\" $pem" >&2
    echo "      (macOS asks for your password once. In an SSH session or a VM without a" >&2
    echo "       screen, use: sudo security add-trusted-cert -d -r trustRoot -p codeSign -k \"$kc\" $pem)" >&2
    if ! keychain_listed "$kc"; then
        echo "  security list-keychains -d user -s \"$kc\" \"$HOME/Library/Keychains/login.keychain-db\"" >&2
    fi
    echo "  read -r -s -p 'Keychain password: ' KCPW; echo" >&2
    echo "  security set-key-partition-list -S apple-tool:,apple:,codesign: -s -t private -l \"$CUA_LOCAL_SIGN_CN\" -k \"\$KCPW\" \"$kc\" >/dev/null; unset KCPW" >&2
    echo "      (-l limits it to that certificate's key; do not drop it: without it every signing key in the Keychain is changed)" >&2
    echo "then run this script again, or run it with --setup-signing and it does this for you." >&2
}

# Whether codesign can sign with `identity` now, with no prompt: a throwaway
# copy of a Mach-O file of the bundle, signed. (That the signature is
# certificate-backed is checked by sign_staged_app, as before.)
signing_probe() {
    local identity="$1" macho="$2" dir probe status=0
    dir="$(mktemp -d "${TMPDIR:-/tmp}/cua-local-sign-probe.XXXXXX")" || return 1
    probe="$dir/probe"
    if cp "$macho" "$probe"; then
        codesign_bounded 20 --force --sign "$identity" "$probe" >/dev/null 2>&1 || status=$?
    else
        status=1
    fi
    rm -rf "$dir"
    [ "$status" = 0 ]
}

# The certificate `identity` (its SHA-1) as a PEM file at `out`.
export_signing_certificate() {
    security find-certificate -a -Z -p -c "$CUA_LOCAL_SIGN_CN" "$2" 2>/dev/null | awk -v want="$1" '
        /^SHA-1 hash:/ { found = (toupper($3) == toupper(want)); next }
        /^-----BEGIN CERTIFICATE-----/ { if (found) pem = 1 }
        pem { print }
        /^-----END CERTIFICATE-----/ { pem = 0; found = 0 }
    ' > "$3"
    grep -q 'BEGIN CERTIFICATE' "$3"
}

# A new Keychain: the certificate is there (the shared helper creates it) but
# codesign cannot use it until macOS trusts it for code signing, its Keychain
# is in the search list and codesign may use its key. This checks that with a
# real signing, and when it does not work says so and, with consent
# (--setup-signing, or "y" at a terminal), does the setup. It never picks
# another certificate, never touches one chosen with
# CUA_DRIVER_LOCAL_SIGNING_IDENTITY, and never trusts anything unasked.
# Fails (and has printed why and what to type) when the identity is still not
# usable; sign_staged_app then decides as it always did (required: stop;
# otherwise ad hoc, reported).
make_signing_identity_usable() {
    local identity="$1" app_stage="$2" macho kc answer pem pw line
    local current=()
    macho="$app_stage/Contents/MacOS/$(plist_get "$app_stage/Contents/Info.plist" CFBundleExecutable || true)"
    [ -f "$macho" ] || macho="$app_stage/Contents/MacOS/cua"
    [ -f "$macho" ] || return 0
    signing_probe "$identity" "$macho" && return 0

    if [ -n "${CUA_DRIVER_LOCAL_SIGNING_IDENTITY:-}" ]; then
        warn "codesign cannot use the identity you chose ($CUA_DRIVER_LOCAL_SIGNING_IDENTITY) yet: trust it for code signing and let codesign use its key. The script does not change a certificate you picked."
        return 1
    fi
    kc="$(local_signing_keychain)"
    warn "codesign cannot use the stable local certificate yet (a new Keychain: it is not trusted for code signing, or codesign may not use its key)."

    answer=n
    if [ "$SETUP_SIGNING" = 1 ]; then
        answer=y
    elif [ -t 0 ] && [ -t 1 ]; then
        printf 'Set it up now? macOS asks for your password once to trust the certificate. [Y/n] '
        read -r answer || answer=n
        case "$answer" in ""|y|Y|yes|YES|Yes) answer=y ;; *) answer=n ;; esac
    fi
    if [ "$answer" != y ]; then
        print_signing_bootstrap
        return 1
    fi

    # 1. The Keychain in the search list (the trust check looks there).
    if ! keychain_listed "$kc"; then
        while IFS= read -r line; do
            line="${line#"${line%%[![:space:]]*}"}"
            line="${line#\"}"
            line="${line%\"}"
            [ -z "$line" ] || current+=("$line")
        done < <(security list-keychains -d user 2>/dev/null)
        security list-keychains -d user -s "$kc" ${current[@]+"${current[@]}"} \
            || { err "could not add $kc to the Keychain search list"; print_signing_bootstrap; return 1; }
        ok "added $kc to the Keychain search list"
    fi
    # 2. Trusted for code signing: one macOS prompt (user trust settings).
    if ! identity_trusted "$identity" "$kc"; then
        pem="$(mktemp "${TMPDIR:-/tmp}/cua-local-signing.XXXXXX")" || return 1
        if ! export_signing_certificate "$identity" "$kc" "$pem"; then
            rm -f "$pem"
            err "could not read the certificate $identity from $kc"
            print_signing_bootstrap
            return 1
        fi
        log "trusting the certificate for code signing (macOS asks for your password)"
        if ! security add-trusted-cert -r trustRoot -p codeSign -k "$kc" "$pem"; then
            rm -f "$pem"
            err "macOS did not trust the certificate (it asks in a window on this Mac's screen)."
            print_signing_bootstrap
            return 1
        fi
        rm -f "$pem"
        ok "the certificate is trusted for code signing"
    fi
    if signing_probe "$identity" "$macho"; then
        ok "codesign can use the stable local certificate"
        return 0
    fi
    # 3. codesign may use the key without a prompt. Needs the Keychain's
    # password: asked here, used once, not stored.
    if [ -t 0 ]; then
        printf 'Password of the Keychain %s (used once, not stored): ' "$kc"
        read -r -s pw || pw=""
        echo
        if [ -n "$pw" ] && open_key_to_codesign "$kc" "$pw"; then
            pw=""
            if signing_probe "$identity" "$macho"; then
                ok "codesign can use the stable local certificate"
                return 0
            fi
        fi
        pw=""
        err "codesign still cannot use the certificate."
    else
        err "codesign may not use the certificate's key yet, and the Keychain password is asked at a terminal."
    fi
    print_signing_bootstrap
    return 1
}

# Signs the staged app without touching the live one. The stable local
# certificate first; failing that, an ad hoc signature (the permissions then
# reset on the next rebuild), unless one is required or the live app already
# has the certificate (never swap a working signature for an ad hoc one).
sign_staged_app() {
    local app_stage="$1" app_dest="$2" sign_id requirement
    sign_id="$(ensure_local_signing_identity)"
    if [ "$sign_id" != "-" ]; then
        make_signing_identity_usable "$sign_id" "$app_stage" || true
    fi

    if [ "$sign_id" != "-" ]; then
        if sign_bundle_with "$sign_id" "$app_stage"; then
            requirement="$(designated_requirement "$app_stage")"
            if [ "$(classify_designated_requirement "$requirement")" = "certificate-backed" ]; then
                ok "signed the staged app with the stable local certificate: its permissions survive rebuilds"
                return 0
            fi
            warn "the stable identity produced a signature that is not certificate-backed"
        else
            warn "signing with the stable local identity failed"
        fi
    fi

    if [ "$REQUIRE_STABLE_SIGNING" = 1 ]; then
        clean_partial_bundle_signature "$app_stage"
        err "stable signing is required, but no usable certificate-backed identity was available. The installed app was not changed."
        print_signing_bootstrap
        return 1
    fi

    if [ -d "$app_dest" ]; then
        requirement="$(designated_requirement "$app_dest")"
        if [ "$(classify_designated_requirement "$requirement")" = "certificate-backed" ]; then
            clean_partial_bundle_signature "$app_stage"
            err "stable signing failed; keeping the installed certificate-signed $app_dest and its permissions."
            print_signing_bootstrap
            return 1
        fi
    fi

    clean_partial_bundle_signature "$app_stage"
    if ! sign_bundle_with - "$app_stage"; then
        clean_partial_bundle_signature "$app_stage"
        err "could not sign the staged app; the installed app was not changed."
        return 1
    fi
    requirement="$(designated_requirement "$app_stage")"
    if [ "$(classify_designated_requirement "$requirement")" != "ad-hoc" ]; then
        err "could not verify the staged app's ad hoc requirement; the installed app was not changed."
        return 1
    fi
    warn "$LOCAL_APP_DIR_NAME is signed ad hoc: its Screen Recording and Accessibility grants reset on the next rebuild."
    print_signing_bootstrap
}

# Only when an ad hoc cdhash changed, and only for the Local bundle id (the
# stale TCC rows would otherwise keep the old hash and the app never prompts).
reset_spaces_local_tcc() {
    local previous="$1" installed="$2" service failed=""
    ad_hoc_requirement_changed "$previous" "$installed" || return 0
    if ! command -v tccutil >/dev/null 2>&1; then
        warn "tccutil is missing: reset the Screen Recording and Accessibility rows of $LOCAL_BUNDLE_ID in System Settings."
        return 0
    fi
    for service in Accessibility ScreenCapture; do
        tccutil reset "$service" "$LOCAL_BUNDLE_ID" >/dev/null 2>&1 || failed="$failed $service"
    done
    if [ -n "$failed" ]; then
        warn "could not reset these permissions for $LOCAL_BUNDLE_ID:$failed (tccutil reset <service> $LOCAL_BUNDLE_ID)."
    else
        warn "the ad hoc signature changed: cleared the old Accessibility and Screen Recording rows of $LOCAL_BUNDLE_ID. Grant them again when asked."
    fi
}

# What the Local bundle must be, whether it is staged or installed: signed, the
# Local identity and scheme, and none of the Stable ones (bundle id, scheme,
# helper label), no update feed. Prints every problem; fails on any.
verify_local_bundle() {
    local app="$1" plist="$1/Contents/Info.plist" failures=0 id channel label schemes scheme
    local has_local_scheme=0 helper_plist associated i

    bad() { err "$app: $*"; failures=$((failures + 1)); }

    if ! codesign --verify --deep --strict "$app" 2>/dev/null; then
        bad "the code signature does not verify"
    fi
    [ -f "$plist" ] || { bad "no Contents/Info.plist"; return 1; }

    id="$(plist_get "$plist" CFBundleIdentifier || true)"
    [ "$id" = "$LOCAL_BUNDLE_ID" ] || bad "bundle id is '${id:-none}', expected $LOCAL_BUNDLE_ID"

    if plist_get "$plist" SUFeedURL >/dev/null; then bad "it has an update feed (SUFeedURL)"; fi
    if plist_get "$plist" SUPublicEDKey >/dev/null; then bad "it has an update key (SUPublicEDKey)"; fi

    schemes="$(plist_url_schemes "$plist")"
    while IFS= read -r scheme; do
        [ -n "$scheme" ] || continue
        [ "$scheme" != "$LOCAL_URL_SCHEME" ] || has_local_scheme=1
        [ "$scheme" != "$STABLE_URL_SCHEME" ] || bad "it registers the Stable URL scheme $STABLE_URL_SCHEME://"
    done <<EOF
$schemes
EOF
    [ "$has_local_scheme" = 1 ] || bad "it does not register the URL scheme $LOCAL_URL_SCHEME://"

    # The host service is this checkout's build, not the pinned release (which
    # would ignore the Local ports and take the released app's).
    if [ ! -x "$app/Contents/Helpers/cua-spacesd" ]; then
        bad "it has no Contents/Helpers/cua-spacesd"
    elif [ "$(code_hash "$app/Contents/Helpers/cua-spacesd" || true)" != "$SPACESD_CODE_HASH" ]; then
        bad "the bundled cua-spacesd is not the build of this checkout ($SPACESD_BIN), probably the pinned release, which ignores the Local ports. build-app.sh must bundle \$CUA_SPACESD_BIN."
    fi

    # The New UI the app shows: build-app.sh copies the web build in, and the
    # app says "The new UI is not in this build" without it.
    [ -f "$app/Contents/Resources/WebUI/index.html" ] \
        || bad "it has no web UI (Contents/Resources/WebUI/index.html): the app would say \"The new UI is not in this build\""

    channel="$(plist_get "$plist" CuaChannel || true)"
    [ "$channel" = local ] || bad "CuaChannel is '${channel:-none}', expected local"
    if label="$(plist_get "$plist" CuaHelperLabel)" && [ "$label" != "$LOCAL_HELPER_LABEL" ]; then
        bad "CuaHelperLabel is '$label', expected $LOCAL_HELPER_LABEL"
    fi

    # The login item helper: its launchd label and the app it belongs to.
    for helper_plist in "$app"/Contents/Library/LaunchAgents/*.plist; do
        [ -f "$helper_plist" ] || continue
        label="$(plist_get "$helper_plist" Label || true)"
        [ "$label" != "$STABLE_HELPER_LABEL" ] || bad "$(basename "$helper_plist") has the Stable helper label $STABLE_HELPER_LABEL"
        # Each entry is compared whole: the Local id is Stable's plus ".local",
        # so a substring test would flag the Local id itself. launchd takes the
        # key as an array or as one string; both are read.
        associated="$(plist_get "$helper_plist" AssociatedBundleIdentifiers || true)"
        case "$associated" in
            "Array {"*)
                i=0
                while associated="$(plist_get "$helper_plist" "AssociatedBundleIdentifiers:$i")"; do
                    [ "$associated" != "$STABLE_BUNDLE_ID" ] || bad "$(basename "$helper_plist") is associated with the Stable app $STABLE_BUNDLE_ID"
                    i=$((i + 1))
                done
                ;;
            *)
                [ "$associated" != "$STABLE_BUNDLE_ID" ] || bad "$(basename "$helper_plist") is associated with the Stable app $STABLE_BUNDLE_ID"
                ;;
        esac
    done

    [ "$failures" = 0 ]
}

log "signing"
PREVIOUS_REQUIREMENT=""
if [ -d "$APP_DEST" ]; then
    PREVIOUS_REQUIREMENT="$(designated_requirement "$APP_DEST" || true)"
fi
sign_staged_app "$STAGE_APP" "$APP_DEST" || exit 1
STAGED_REQUIREMENT="$(designated_requirement "$STAGE_APP")"
STAGED_CLASS="$(classify_designated_requirement "$STAGED_REQUIREMENT")"

log "verifying the staged app"
if ! verify_local_bundle "$STAGE_APP"; then
    die "the staged app is not a valid Cua Spaces Local bundle; the installed app was not changed."
fi

# --- Swap ------------------------------------------------------------------------

log "installing $APP_DEST"
# The path may have been taken since the preflight.
require_local_bundle_or_absent "$APP_DEST" || exit 1
# The old Local (bundle id com.trycua.spaces.local) is retired here, after the
# build and its checks, so a failed build leaves it as it was. Its settings in
# ~/Library are removed below, once the new app is in.
REPLACING_LEGACY_LOCAL=0
LEGACY_HELPER_WAS_ENABLED=0
if is_legacy_local_bundle "$APP_DEST"; then
    REPLACING_LEGACY_LOCAL=1
    PREVIOUS_REQUIREMENT=""
    log "found the old ${LOCAL_APP_DIR_NAME%.app} (bundle id $LOCAL_LEGACY_BUNDLE_ID): replacing it with the renamed app ($LOCAL_BUNDLE_ID) and removing its old settings. Your sign-in stays; macOS asks for the app's permissions once more."
    if local_helper_enabled "$APP_DEST"; then LEGACY_HELPER_WAS_ENABLED=1; fi
    retire_local_app "$APP_DEST"
    if [ -x "$LSREGISTER" ]; then
        "$LSREGISTER" -u "$APP_DEST" >/dev/null 2>&1 || true
    fi
fi
quit_local_app "$APP_DEST"
mkdir -p "$APP_PARENT"
APP_BACKUP="$APP_DEST.install-backup.$$"
rm -rf "$APP_BACKUP"
if [ -d "$APP_DEST" ]; then
    mv "$APP_DEST" "$APP_BACKUP"
fi
install_valid=false
INSTALLED_REQUIREMENT=""
INSTALLED_CLASS=""
if ditto "$STAGE_APP" "$APP_DEST" && verify_local_bundle "$APP_DEST"; then
    INSTALLED_REQUIREMENT="$(designated_requirement "$APP_DEST")"
    INSTALLED_CLASS="$(classify_designated_requirement "$INSTALLED_REQUIREMENT")"
    if [ "$INSTALLED_REQUIREMENT" = "$STAGED_REQUIREMENT" ] \
        && [ "$INSTALLED_CLASS" = "$STAGED_CLASS" ] \
        && [ "$INSTALLED_CLASS" != unknown ]; then
        install_valid=true
    fi
fi
if [ "$install_valid" = true ]; then
    rm -rf "$APP_BACKUP"
else
    rm -rf "$APP_DEST"
    if [ -d "$APP_BACKUP" ]; then
        mv "$APP_BACKUP" "$APP_DEST"
        if [ "$REPLACING_LEGACY_LOCAL" = 1 ]; then
            restore_retired_local_app "$APP_DEST" "$LEGACY_HELPER_WAS_ENABLED"
        fi
        die "the installed copy did not verify; restored the previous $LOCAL_APP_DIR_NAME."
    fi
    die "the installed copy did not verify and was removed."
fi
ok "installed $APP_DEST"
if [ "$REPLACING_LEGACY_LOCAL" = 1 ]; then
    remove_legacy_local_data >/dev/null
fi
if [ "$INSTALLED_CLASS" = certificate-backed ]; then
    ok "signature: certificate-backed, the same on every rebuild"
fi

# Register the new bundle with LaunchServices now (it does it late otherwise):
# opening it and the cua-local:// scheme resolve to this copy.
if [ -x "$LSREGISTER" ]; then
    "$LSREGISTER" -f "$APP_DEST" >/dev/null 2>&1 || true
fi
reset_spaces_local_tcc "$PREVIOUS_REQUIREMENT" "$INSTALLED_REQUIREMENT"

# --- The CLI -----------------------------------------------------------------------

BUNDLED_CLI="$APP_DEST/Contents/MacOS/cua"
if [ -x "$BUNDLED_CLI" ]; then
    mkdir -p "$BIN_DIR"
    # -n: replace the link itself, never write through it.
    ln -sfn "$BUNDLED_CLI" "$CLI_LINK"
    ok "$CLI_LINK -> $BUNDLED_CLI"
else
    warn "the bundle has no cua CLI (cargo build --release -p cua-spaces-cli, then run this again): $LOCAL_CLI_NAME was not linked."
fi

# --- lume-local ----------------------------------------------------------------------

if [ "$WITH_LUME_LOCAL" = 1 ]; then
    log "installing lume-local"
    lume_args=()
    [ "$CONFIG" != release ] || lume_args+=(--release)
    /bin/bash "$LUME_INSTALL" ${lume_args[@]+"${lume_args[@]}"}
fi

# --- Done ------------------------------------------------------------------------------

echo ""
ok "Cua Spaces Local is installed. The released Cua Spaces, ~/.cua and ~/.local/bin/cua were not touched."
case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) echo "  $BIN_DIR is not on your PATH: add it to use $LOCAL_CLI_NAME (no shell profile was edited)." ;;
esac
if [ "$LAUNCH" = 1 ]; then
    log "opening $APP_DEST"
    open "$APP_DEST" || warn "could not open the app"
fi
