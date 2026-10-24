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
#                            [--require-stable-signing]
#
#   --release            build the release configuration (default: debug)
#   --no-launch          do not open the app when it is installed
#   --prefix DIR         install under DIR instead of $HOME: the app in
#                        DIR/Applications, `cua-local` in DIR/.local/bin
#                        (the state, ~/.cua-local, stays in $HOME)
#   --with-lume-local    also install lume-local (libs/lume/scripts/install-local.sh)
#   --skip-rust-build    do not run cargo and stage the library: use the ones
#                        already built
#   --require-stable-signing
#                        stop unless the app can be signed with the stable
#                        local certificate (no ad hoc fallback)
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
# Signing: the stable local certificate shared with Cua Driver's local install
# (libs/cua-driver/scripts/_local-signing.sh), so the Screen Recording and
# Accessibility grants of Cua Spaces Local survive a rebuild. Without it the
# bundle is signed ad hoc and the grants reset on every rebuild (reported).
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
APP_PARENT="$PREFIX/Applications"
APP_DEST="$APP_PARENT/$LOCAL_APP_DIR_NAME"
BIN_DIR="$PREFIX/.local/bin"
CLI_LINK="$BIN_DIR/$LOCAL_CLI_NAME"

# The two paths this script writes outside the build tree. Their names are the
# Local ones and neither is under /Applications: a mistake in the prefix cannot
# aim them at the Stable app or CLI.
[ "$(basename "$APP_DEST")" = "$LOCAL_APP_DIR_NAME" ] || die "internal error: app path $APP_DEST"
[ "$(basename "$CLI_LINK")" = "$LOCAL_CLI_NAME" ] || die "internal error: CLI path $CLI_LINK"
case "$APP_DEST" in
    /Applications/*) die "refusing to install Cua Spaces Local into /Applications (that is where Cua Spaces lives): use a different --prefix." ;;
esac
if [ "$WITH_LUME_LOCAL" = 1 ] && [ "$PREFIX" != "$HOME" ]; then
    die "--with-lume-local installs under \$HOME and cannot be combined with --prefix."
fi

BUILD_SCRIPT="$SCRIPT_DIR/build-app.sh"
SIGNING_LIB="$REPO_ROOT/libs/cua-driver/scripts/_local-signing.sh"
LUME_INSTALL="$REPO_ROOT/libs/lume/scripts/install-local.sh"
BUILD_OUT="$APP_DIR/.build/local-app"
STAGE_ROOT="$APP_DIR/.build/local-stage"
STAGE_APP="$STAGE_ROOT/$LOCAL_APP_DIR_NAME"

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
if [ "$WITH_LUME_LOCAL" = 1 ]; then
    [ -f "$LUME_INSTALL" ] || die "--with-lume-local: missing $LUME_INSTALL"
    # Without lume-local, that script installs over the Stable lume (the same
    # binary, LaunchAgent, port and VMs).
    if ! grep -q "$LOCAL_LUME_BIN_NAME" "$LUME_INSTALL"; then
        die "--with-lume-local: $LUME_INSTALL in this checkout still installs over the Stable lume (it does not know lume-local). Update to a checkout where lume-local has landed."
    fi
fi

printf '%sCua Spaces Local installer%s\n' "$BOLD$BLUE" "$NORMAL"
echo "  source:  $REPO_ROOT"
echo "  config:  $CONFIG"
echo "  app:     $APP_DEST"
echo "  cli:     $CLI_LINK"
echo ""

# --- Build -----------------------------------------------------------------------

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
fi

log "building the app (build-app.sh --channel local $CONFIG)"
mkdir -p "$BUILD_OUT"
# build-app.sh prints the bundle path as its last line.
if ! BUILT_APP="$("$BUILD_SCRIPT" --channel local "$CONFIG" "$BUILD_OUT" | tee /dev/stderr | tail -n 1)"; then
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
# com.trycua.cua-env-driver for cua-spacesd; only the identity changes.
sign_piece() {
    local identity="$1" target="$2" preserve="$3" out
    local args=(--force --sign "$identity")
    [ -z "$preserve" ] || args+=("--preserve-metadata=$preserve")
    if ! out="$(codesign_bounded 20 "${args[@]}" "$target" 2>&1)"; then
        err "codesign failed for $target"
        printf '%s\n' "$out" >&2
        return 1
    fi
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

print_signing_bootstrap() {
    echo "Cua Spaces Local needs the stable local signing certificate (the one Cua Driver's" >&2
    echo "local install uses) to keep its permissions across rebuilds. To set it up:" >&2
    echo "  SIGNING_KEYCHAIN=\"\$HOME/Library/Keychains/cua-driver-signing.keychain-db\"" >&2
    echo "  security create-keychain \"\$SIGNING_KEYCHAIN\"  # first time only; prompts for a password" >&2
    echo "  security set-keychain-settings \"\$SIGNING_KEYCHAIN\"" >&2
    echo "  security unlock-keychain \"\$SIGNING_KEYCHAIN\"" >&2
    echo "  export CUA_DRIVER_LOCAL_SIGNING_KEYCHAIN=\"\$SIGNING_KEYCHAIN\"" >&2
    echo "then run this script again. If the certificate is newly created, authorize its private" >&2
    echo "key for codesign as described in libs/cua-driver/scripts/README.md#stable-macos-local-signing" >&2
}

# Signs the staged app without touching the live one. The stable local
# certificate first; failing that, an ad hoc signature (the permissions then
# reset on the next rebuild), unless one is required or the live app already
# has the certificate (never swap a working signature for an ad hoc one).
sign_staged_app() {
    local app_stage="$1" app_dest="$2" sign_id requirement
    sign_id="$(ensure_local_signing_identity)"

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
    local has_local_scheme=0 helper_plist associated

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
        associated="$(plist_get "$helper_plist" AssociatedBundleIdentifiers || true)"
        case "$associated" in
            *"$STABLE_BUNDLE_ID"*) bad "$(basename "$helper_plist") is associated with the Stable app $STABLE_BUNDLE_ID" ;;
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
        die "the installed copy did not verify; restored the previous $LOCAL_APP_DIR_NAME."
    fi
    die "the installed copy did not verify and was removed."
fi
ok "installed $APP_DEST"
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
