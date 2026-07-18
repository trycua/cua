#!/usr/bin/env bash
#
# cua-driver-rs local/debug installer (macOS + Linux). Builds from the
# current source tree and drops the resulting cua-driver binary into the
# same install layout that scripts/install.sh produces — so a local build
# and a release install can coexist + the `current` symlink can flip
# between them.
#
# Private helper — invoked by install-local.sh (the multi-backend
# dispatcher) when the user picks --backend=rust / --experimental-rust
# or runs on a non-macOS host. Do not invoke directly; flag parity with
# the dispatcher's argv shape is maintained from there.
#
# Rust local installer (dev-only helper for libs/cua-driver/rust):
#   --release    build the release configuration (default: debug)
#   --autostart  register an auto-start daemon (macOS: LaunchAgent;
#                Linux: systemd user unit). Default off; the post-install
#                message prints the registration command for the platform.
#
# Not for end-users — scripts/install.sh fetches a built release from
# GitHub. This script is for the developer loop (rapid edit/build/test
# on a Linux or macOS host).
#
# Linux layout produced (matches install.sh):
#
#   ${CUA_DRIVER_HOME:-$HOME/.cua-driver}/packages/
#       releases/<version>-local-<config>-<target>/cua-driver
#       current/cua-driver  -> ../releases/<active>/cua-driver
#   ${CUA_DRIVER_INSTALL_DIR:-$HOME/.local/bin}/cua-driver
#       -> ../current/cua-driver
#
# Legacy env vars `CUA_DRIVER_RS_HOME` / `CUA_DRIVER_RS_INSTALL_DIR` /
# `CUA_DRIVER_RS_BIN_DIR` are still accepted for backwards compat
# (rename from v0.2.16 per PR #1644; this helper was missed in the
# initial rename and ported in PR #1717).
#
# macOS layout produced:
#   /Applications/CuaDriver.app/Contents/MacOS/cua-driver  (bundle replaced wholesale)
#   $HOME/.local/bin/cua-driver -> .../CuaDriver.app/Contents/MacOS/cua-driver
#
# The version string carries `-local-debug` / `-local-release` so it
# never collides with a real release dir and is trivial to GC.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# Rust workspace root: scripts/ is the cross-cutting installer dir at
# libs/cua-driver/scripts/; the Cargo workspace lives one level deeper
# under libs/cua-driver/rust/.
REPO_ROOT="$(cd "$SCRIPT_DIR/../rust" && pwd)"

# --- Load shared daemon-cleanup helpers ---------------------------------
#
# Sibling _install-common.sh defines stop_cua_driver_daemons +
# show_cua_driver_daemon_survivors, mirroring CuaDriverInstall.psm1.
# This is a dev-only path always invoked from a checked-out tree (the
# `install-local.sh` dispatcher only runs from a clone), so the on-disk
# load is the only branch we need — no curl fallback like _install-rust.sh.
# Define no-op stubs if the file is missing so call sites stay
# unconditional and a stale clone doesn't fail the install.
if [ -f "$SCRIPT_DIR/_install-common.sh" ]; then
    # shellcheck source=_install-common.sh
    . "$SCRIPT_DIR/_install-common.sh"
else
    echo "warning: $SCRIPT_DIR/_install-common.sh missing; daemon kill skipped" >&2
    stop_cua_driver_daemons() { :; }
    show_cua_driver_daemon_survivors() { :; }
fi

BOLD=$(tput bold 2>/dev/null || true)
NORMAL=$(tput sgr0 2>/dev/null || true)
RED=$(tput setaf 1 2>/dev/null || true)
GREEN=$(tput setaf 2 2>/dev/null || true)
BLUE=$(tput setaf 4 2>/dev/null || true)
YELLOW=$(tput setaf 3 2>/dev/null || true)

if [ "$(id -u)" -eq 0 ] || [ -n "${SUDO_USER:-}" ]; then
    echo "${RED}Error: do not run this script with sudo or as root.${NORMAL}"
    echo "It prompts for sudo on the specific operations that need it."
    exit 1
fi

# --- Parse arguments ----------------------------------------------------

BUILD_CONFIG="debug"
INSTALL_AUTOSTART=false

while [ "$#" -gt 0 ]; do
    case "$1" in
        --release)
            BUILD_CONFIG="release"
            ;;
        --autostart)
            INSTALL_AUTOSTART=true
            ;;
        --help|-h)
            echo "${BOLD}${BLUE}cua-driver-rs local installer${NORMAL}"
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --release     Build the release configuration (default: debug)."
            echo "  --autostart   Also register a logon-time daemon:"
            echo "                  macOS: LaunchAgent under ~/Library/LaunchAgents"
            echo "                  Linux: systemd --user unit"
            echo "                On macOS this also fixes TCC: a launchd-started daemon"
            echo "                is attributed to com.trycua.driver (not your terminal),"
            echo "                so you grant Accessibility + Screen Recording once and"
            echo "                every cua-driver call/mcp routes through it correctly."
            echo "  --help        Show this help."
            echo ""
            echo "Examples:"
            echo "  $0                       # debug build, install junction layout"
            echo "  $0 --release             # release build"
            echo "  $0 --release --autostart # release + daemon at logon"
            exit 0
            ;;
        *)
            echo "${RED}Unknown option: $1${NORMAL}"
            echo "Use --help for usage."
            exit 1
            ;;
    esac
    shift
done

OS="$(uname -s)"
ARCH="$(uname -m)"
case "$OS" in
    Darwin) TARGET_TRIPLE="${ARCH}-apple-darwin" ;;
    Linux)  TARGET_TRIPLE="${ARCH}-unknown-linux-gnu" ;;
    *)      echo "${RED}Unsupported OS: $OS${NORMAL}"; exit 1 ;;
esac

# Canonical home is `~/.cua-driver/` (renamed from `~/.cua-driver-rs/`
# in v0.2.16 — PR #1644). Accept the legacy `CUA_DRIVER_RS_HOME` env var
# too so any dev scripts that still set it keep working.
HOME_DIR="${CUA_DRIVER_HOME:-${CUA_DRIVER_RS_HOME:-$HOME/.cua-driver}}"
BIN_DIR="${CUA_DRIVER_INSTALL_DIR:-${CUA_DRIVER_BIN_DIR:-${CUA_DRIVER_RS_INSTALL_DIR:-${CUA_DRIVER_RS_BIN_DIR:-$HOME/.local/bin}}}}"
RELEASES_DIR="$HOME_DIR/packages/releases"
CURRENT_LINK="$HOME_DIR/packages/current"

# Best-effort sweep: if a previous install left a stale `~/.cua-driver-rs/`
# (the pre-v0.2.16 home), remove it. The runtime sweeps it on first call
# too (see telemetry.rs::migrate_legacy_telemetry_home) so this is belt-
# and-braces — keeps the home dir layout single-rooted for the user.
LEGACY_HOME_DIR="$HOME/.cua-driver-rs"
if [ -d "$LEGACY_HOME_DIR" ] && [ "$HOME_DIR" != "$LEGACY_HOME_DIR" ]; then
    mkdir -p "$HOME_DIR"
    for telemetry_file in .telemetry_id .installation_recorded; do
        if [ -f "$LEGACY_HOME_DIR/$telemetry_file" ] && [ ! -e "$HOME_DIR/$telemetry_file" ]; then
            cp -p "$LEGACY_HOME_DIR/$telemetry_file" "$HOME_DIR/$telemetry_file" 2>/dev/null \
                && echo "  Preserved legacy telemetry state $telemetry_file" \
                || echo "  Could not preserve legacy telemetry state $telemetry_file"
        fi
    done
    echo "  Sweeping legacy install dir $LEGACY_HOME_DIR"
    rm -rf "$LEGACY_HOME_DIR"
fi

VERSION_TAG="0.0.0-local-$BUILD_CONFIG"
VERSIONED_DIR="$RELEASES_DIR/$VERSION_TAG-$TARGET_TRIPLE"

echo "${BOLD}${BLUE}cua-driver-rs local installer${NORMAL}"
echo "  source:  ${BOLD}$REPO_ROOT${NORMAL}"
echo "  config:  ${BOLD}$BUILD_CONFIG${NORMAL}"
echo "  target:  ${BOLD}$TARGET_TRIPLE${NORMAL}"
echo "  bin:     ${BOLD}$BIN_DIR/cua-driver${NORMAL}"
echo "  current: ${BOLD}$CURRENT_LINK${NORMAL}"
echo ""

# --- Prerequisites ------------------------------------------------------

if ! command -v cargo >/dev/null 2>&1; then
    # Common rustup default install at $HOME/.cargo/bin/cargo — source the
    # rustup-shipped env script if present so cargo + rustc + the active
    # toolchain shims all land on PATH for the rest of this script. This
    # matters because rustup-init writes the PATH-prepending line into the
    # user's shell rc, which only takes effect in NEW interactive shells —
    # a fresh post-rustup invocation of `./install-local.sh` in the same
    # shell as the rustup install would otherwise fail here even though
    # cargo is on disk.
    if [ -f "$HOME/.cargo/env" ]; then
        # shellcheck disable=SC1091
        . "$HOME/.cargo/env"
    elif [ -x "$HOME/.cargo/bin/cargo" ]; then
        # Older rustup installs (or non-rustup Cargo installs) may lack
        # the env script — directly prepend the canonical bin dir.
        export PATH="$HOME/.cargo/bin:$PATH"
    fi
fi
if ! command -v cargo >/dev/null 2>&1; then
    echo "${RED}Error: cargo not found on PATH.${NORMAL}"
    echo "Install Rust via rustup: https://rustup.rs/"
    echo "After install, either open a new shell or run: . \$HOME/.cargo/env"
    exit 1
fi

# --- Build --------------------------------------------------------------

echo "${BOLD}Building cua-driver ($BUILD_CONFIG)...${NORMAL}"
cd "$REPO_ROOT"
if [ "$BUILD_CONFIG" = "release" ]; then
    cargo build --release -p cua-driver
else
    cargo build -p cua-driver
fi

BUILT_BINARY="$REPO_ROOT/target/$BUILD_CONFIG/cua-driver"
if [ ! -x "$BUILT_BINARY" ]; then
    echo "${RED}Error: build produced no binary at $BUILT_BINARY${NORMAL}"
    exit 1
fi
echo ""

# --- Stage into versioned release dir + repoint `current` --------------

echo "${BOLD}Staging into $VERSIONED_DIR${NORMAL}"
mkdir -p "$VERSIONED_DIR"
cp "$BUILT_BINARY" "$VERSIONED_DIR/cua-driver"
chmod +x "$VERSIONED_DIR/cua-driver"

# Re-sign with a fresh ad-hoc signature.
#
# macOS 26+ Taskgated rejects the linker-emitted ad-hoc signature once
# the binary has been copied (the kernel's cached signature for the new
# inode doesn't match the embedded one strictly enough for the newer
# CODESIGNING namespace). Result is `SIGKILL (Code Signature Invalid)
# — Taskgated Invalid Signature` on first run, no stderr output, exit
# code 137 — extremely confusing without a diagnostic-report dig. The
# fix: re-sign in place. `codesign --force --sign -` emits a fresh
# ad-hoc signature keyed to the new on-disk bytes, which Taskgated
# accepts. Cheap (~50ms on a 40MB binary). macOS-only — no-op on Linux.
if [ "$OS" = "Darwin" ]; then
    if command -v codesign >/dev/null 2>&1; then
        codesign --force --sign - "$VERSIONED_DIR/cua-driver" 2>/dev/null \
            || echo "${YELLOW}warning: codesign --force --sign - failed; first run may fail with SIGKILL on macOS 26+${NORMAL}" >&2
    fi
fi

SOURCE_SKILLS="$REPO_ROOT/Skills/cua-driver"

# Atomically point `current` at the new versioned release dir.
#
# Previous version used `ln -s … current.new` + `mv -Tf current.new current`
# with a BSD `mv -f` fallback. The BSD fallback path is broken: when the
# destination is a symlink-to-directory, BSD `mv` *follows* it and drops
# the temp symlink INSIDE the directory as `current/current.new`, leaving
# stale `current.new` orphans at both levels and the actual `current`
# symlink untouched. macOS doesn't ship GNU `mv` so the `-Tf` path never
# fires on this host.
#
# `ln -sfn` is the POSIX primitive that does what we wanted from the
# start: replace the existing symlink atomically, without dereferencing.
# Works the same on macOS BSD and Linux GNU coreutils. No temp file
# means no orphan to clean up on partial failure.
mkdir -p "$HOME_DIR/packages"
# Sweep any orphan temp from a previous (pre-fix) run before re-creating.
rm -f "$CURRENT_LINK.new"
ln -sfn "$VERSIONED_DIR" "$CURRENT_LINK"
echo "${GREEN}current -> $VERSIONED_DIR${NORMAL}"
echo ""

# --- macOS: stable local code-signing identity (so TCC grants survive rebuilds) ---
#
# Ad-hoc signing (`codesign --sign -`) keys the TCC grant
# (Accessibility / Screen Recording) on the binary's *cdhash*, which changes
# on EVERY rebuild — so the grant silently invalidates on each install-local
# and the daemon re-prompts ("I already granted!"). Signing with a certificate
# keys the grant on the cert identity instead, which is stable across rebuilds.
# We create a self-signed code-signing cert once (idempotent, in the login
# keychain by default) and reuse it. A maintainer VM may point
# CUA_DRIVER_LOCAL_SIGNING_KEYCHAIN at a dedicated, already-unlocked keychain;
# this avoids image-specific login-keychain ACL failures. Local dev only;
# releases are CI-signed.
#
# Echoes the `codesign --sign` argument: the valid identity's SHA-1 when
# available, or "-" when it can't be created — no codesign/openssl, CI,
# locked keychain. Use the identity hash rather than the certificate name:
# stale certificate-only duplicates can share the same name and make codesign
# reject an otherwise valid identity as ambiguous.
CUA_LOCAL_SIGN_CN="CuaDriver Local Signing (cua-driver-rs)"
ensure_local_signing_identity() {
    { [ "$OS" = "Darwin" ] && command -v codesign >/dev/null 2>&1; } || { printf -- '-'; return; }
    local kc="${CUA_DRIVER_LOCAL_SIGNING_KEYCHAIN:-$HOME/Library/Keychains/login.keychain-db}"
    if [ -z "${CUA_DRIVER_LOCAL_SIGNING_KEYCHAIN:-}" ] && [ ! -f "$kc" ]; then
        kc="$HOME/Library/Keychains/login.keychain"
    fi
    [ -f "$kc" ] || { printf -- '-'; return; }
    local identity
    identity="$(security find-identity -v -p codesigning "$kc" 2>/dev/null \
        | awk -v cn="$CUA_LOCAL_SIGN_CN" 'index($0, "\"" cn "\"") { print $2; exit }')"
    if [ -n "$identity" ]; then
        printf '%s' "$identity"; return
    fi
    command -v openssl >/dev/null 2>&1 || { printf -- '-'; return; }
    local tmp; tmp="$(mktemp -d)" || { printf -- '-'; return; }
    printf '[req]\ndistinguished_name=dn\nx509_extensions=ext\nprompt=no\n[dn]\nCN=%s\n[ext]\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=critical,codeSigning\n' \
        "$CUA_LOCAL_SIGN_CN" > "$tmp/req.cnf"
    # Transient password for the p12 handoff (deleted right after import).
    # Apple's `security` rejects the empty-password p12 that openssl 3.x emits
    # by default ("MAC verification failed"), so use a real password + `-legacy`
    # PBE (Apple-compatible). Fall back to non-legacy for LibreSSL/older openssl
    # which lacks `-legacy` but already writes a compatible p12.
    local pw="cua-local-$$"
    if openssl req -x509 -newkey rsa:2048 -keyout "$tmp/key.pem" -out "$tmp/cert.pem" \
            -days 3650 -nodes -config "$tmp/req.cnf" >/dev/null 2>&1 \
       && { openssl pkcs12 -export -legacy -inkey "$tmp/key.pem" -in "$tmp/cert.pem" \
                -out "$tmp/id.p12" -passout pass:"$pw" -name "$CUA_LOCAL_SIGN_CN" >/dev/null 2>&1 \
            || openssl pkcs12 -export -inkey "$tmp/key.pem" -in "$tmp/cert.pem" \
                -out "$tmp/id.p12" -passout pass:"$pw" -name "$CUA_LOCAL_SIGN_CN" >/dev/null 2>&1; } \
       && security import "$tmp/id.p12" -k "$kc" -P "$pw" -A -T /usr/bin/codesign >/dev/null 2>&1; then
        identity="$(security find-identity -v -p codesigning "$kc" 2>/dev/null \
            | awk -v cn="$CUA_LOCAL_SIGN_CN" 'index($0, "\"" cn "\"") { print $2; exit }')"
        rm -rf "$tmp"
        if [ -n "$identity" ]; then
            printf '%s' "$identity"; return
        fi
        printf -- '-'; return
    fi
    rm -rf "$tmp"
    printf -- '-'
}

# Keychain-backed codesign can wait forever for a GUI authorization prompt when
# install-local is launched from a headless shell. Keep the install bounded;
# ad-hoc signing remains a usable fallback for local development.
codesign_bounded() {
    local timeout_seconds="$1"
    shift
    if command -v gtimeout >/dev/null 2>&1; then
        if [ -n "${CUA_DRIVER_LOCAL_SIGNING_KEYCHAIN:-}" ]; then
            gtimeout "$timeout_seconds" codesign \
                --keychain "$CUA_DRIVER_LOCAL_SIGNING_KEYCHAIN" "$@"
        else
            gtimeout "$timeout_seconds" codesign "$@"
        fi
    elif command -v perl >/dev/null 2>&1; then
        if [ -n "${CUA_DRIVER_LOCAL_SIGNING_KEYCHAIN:-}" ]; then
            perl -e 'alarm shift; exec @ARGV' "$timeout_seconds" codesign \
                --keychain "$CUA_DRIVER_LOCAL_SIGNING_KEYCHAIN" "$@"
        else
            perl -e 'alarm shift; exec @ARGV' "$timeout_seconds" codesign "$@"
        fi
    else
        if [ -n "${CUA_DRIVER_LOCAL_SIGNING_KEYCHAIN:-}" ]; then
            codesign --keychain "$CUA_DRIVER_LOCAL_SIGNING_KEYCHAIN" "$@"
        else
            codesign "$@"
        fi
    fi
}

clean_partial_bundle_signature() {
    local app="$1"
    # A certificate-backed codesign killed by the timeout can leave both a
    # partial resource seal and `<executable>.cstemp`. Signing over that state
    # seals the transient file; when the interrupted signer removes it later,
    # the fallback bundle becomes invalid. Always restart fallback signing from
    # the unsigned staged bundle.
    rm -rf "$app/Contents/_CodeSignature"
    find "$app" -type f -name '*.cstemp' -delete
}

# --- macOS: wrap the binary in CuaDriver.app for a stable TCC identity ---
#
# TCC keys Accessibility / Screen-Recording grants on the bundle
# identifier (com.trycua.driver), not the bare executable path. A loose
# binary gets grants attributed to its ad-hoc cdhash, which changes on
# every rebuild — so permissions silently reset and never appear cleanly
# under System Settings. Mirror the production path (install.sh) + the CD
# bundle-assembly step: drop the freshly built binary into the checked-in
# CuaDriverBundle skeleton, install the bundle to /Applications, and point
# the visible bin at the binary INSIDE the bundle. Linux/Windows have no
# .app concept and keep the bare-binary symlink below.
APP_DEST="/Applications/CuaDriver.app"
if [ "$OS" = "Darwin" ]; then
    SKELETON="$REPO_ROOT/scripts/CuaDriverBundle"
    if [ ! -d "$SKELETON/Contents" ]; then
        echo "${RED}Error: bundle skeleton missing at $SKELETON${NORMAL}" >&2
        exit 1
    fi
    APP_STAGE="$VERSIONED_DIR/CuaDriver.app"
    rm -rf "$APP_STAGE"
    mkdir -p "$APP_STAGE/Contents/MacOS"
    cp -R "$SKELETON/Contents/." "$APP_STAGE/Contents/"
    cp "$VERSIONED_DIR/cua-driver" "$APP_STAGE/Contents/MacOS/cua-driver"
    chmod +x "$APP_STAGE/Contents/MacOS/cua-driver"
    rm -f "$APP_STAGE/Contents/MacOS/.gitkeep"
    # Stamp the local build version so the bundle reports something sane.
    if command -v plutil >/dev/null 2>&1; then
        plutil -replace CFBundleShortVersionString -string "$VERSION_TAG" \
            "$APP_STAGE/Contents/Info.plist" 2>/dev/null || true
        plutil -replace CFBundleVersion -string "$VERSION_TAG" \
            "$APP_STAGE/Contents/Info.plist" 2>/dev/null || true
    fi
    # Sign the staged bundle before touching the live installation. Required on
    # macOS 26+ where Taskgated rejects a copied binary's stale signature.
    # Prefer the STABLE self-signed identity so TCC grants survive rebuilds;
    # never downgrade an existing certificate-signed installation to ad-hoc,
    # because that would invalidate its working TCC grants.
    if command -v codesign >/dev/null 2>&1; then
        SIGN_ID="$(ensure_local_signing_identity)"
        if [ "$SIGN_ID" != "-" ] \
           && codesign_bounded 20 --force --deep --sign "$SIGN_ID" "$APP_STAGE" 2>/dev/null; then
            echo "${GREEN}signed staged app with a stable local identity — TCC grants survive future install-local rebuilds${NORMAL}"
        elif [ -d "$APP_DEST" ] \
             && codesign -d -r- "$APP_DEST" 2>&1 | grep -q 'certificate leaf'; then
            echo "${RED}Error: stable signing failed; preserving the existing certificate-signed $APP_DEST and its TCC grants.${NORMAL}" >&2
            echo "Unlock/authorize the configured signing-keychain key, then rerun install-local." >&2
            exit 1
        else
            clean_partial_bundle_signature "$APP_STAGE"
            if codesign_bounded 20 --force --deep --sign - "$APP_STAGE" 2>/dev/null; then
                if [ "$SIGN_ID" != "-" ]; then
                    echo "${YELLOW}note: stable-identity signing failed; signed ad-hoc instead (Accessibility/Screen Recording will reset on the next rebuild)${NORMAL}" >&2
                fi
            else
                clean_partial_bundle_signature "$APP_STAGE"
                echo "${RED}Error: codesign of staged CuaDriver.app failed; live installation was not changed.${NORMAL}" >&2
                exit 1
            fi
        fi
        if ! codesign --verify --deep --strict "$APP_STAGE" 2>/dev/null; then
            echo "${RED}Error: staged CuaDriver.app failed signature verification; live installation was not changed.${NORMAL}" >&2
            exit 1
        fi
    fi

    # Install to /Applications (user-writable for admins; no sudo — same as
    # install.sh). Keep the prior bundle available until the copy completes so
    # an interrupted install cannot leave a corrupt live app.
    APP_BACKUP="${APP_DEST}.install-backup.$$"
    rm -rf "$APP_BACKUP"
    if [ -d "$APP_DEST" ]; then
        mv "$APP_DEST" "$APP_BACKUP"
    fi
    if ditto "$APP_STAGE" "$APP_DEST"; then
        rm -rf "$APP_BACKUP"
    else
        rm -rf "$APP_DEST"
        if [ -d "$APP_BACKUP" ]; then
            mv "$APP_BACKUP" "$APP_DEST"
        fi
        echo "${RED}Error: failed to install CuaDriver.app; restored the previous bundle.${NORMAL}" >&2
        exit 1
    fi
    echo "${GREEN}installed $APP_DEST${NORMAL}"

    # --- Clear a TCC grant pinned to a PREVIOUS signing identity -----------
    #
    # TCC pins each Accessibility / Screen-Recording grant to the app's
    # designated requirement AT GRANT TIME. A user who granted while the app
    # was ad-hoc signed has a grant whose csreq is a bare `cdhash H"..."`
    # (changes every rebuild); a user who granted under a different cert has
    # one pinned to that leaf. After we re-sign with the stable cert above,
    # that old row survives with auth_value=allowed but a csreq that no longer
    # matches THIS build — so the daemon reads "not granted" while System
    # Settings still shows the toggle ON. That's a dead end: re-toggling
    # doesn't help because the row already records a decision, so the grant
    # prompt never re-fires. Detect a signing-identity change vs the last
    # install and `tccutil reset` once, so the next `permissions grant`
    # prompts cleanly and re-pins to the current (stable cert) identity —
    # after which cert-pinned grants survive all future rebuilds.
    #
    # `tccutil reset` needs no sudo / Full Disk Access, and is a no-op when
    # nothing was granted. We only reset when moving TO a cert identity (the
    # case a clean re-grant durably fixes); an ad-hoc build churns its cdhash
    # every rebuild regardless, so resetting it would just add friction.
    if command -v tccutil >/dev/null 2>&1; then
        IDENTITY_MARKER="$HOME_DIR/.tcc-signing-identity"
        NEW_IDENTITY="$(codesign -d -r- "$APP_DEST" 2>&1 \
            | sed -n 's/.*certificate leaf = H"\([0-9a-fA-F]*\)".*/cert:\1/p' | head -1)"
        [ -n "$NEW_IDENTITY" ] || NEW_IDENTITY="adhoc"
        OLD_IDENTITY="$(cat "$IDENTITY_MARKER" 2>/dev/null || true)"
        case "$NEW_IDENTITY" in
            cert:*)
                if [ "$NEW_IDENTITY" != "$OLD_IDENTITY" ]; then
                    tccutil reset Accessibility com.trycua.driver >/dev/null 2>&1 || true
                    tccutil reset ScreenCapture com.trycua.driver >/dev/null 2>&1 || true
                    echo "${BOLD}cleared any stale Accessibility / Screen-Recording grant pinned to a previous build.${NORMAL}"
                    echo "  Grant once more (System Settings → Privacy & Security) and it will${BOLD} stick across every future rebuild${NORMAL} — the grant now pins to a stable signing certificate, not the per-build cdhash."
                fi
                ;;
        esac
        printf '%s\n' "$NEW_IDENTITY" > "$IDENTITY_MARKER" 2>/dev/null || true
    fi
fi

# --- Visible-bin symlink ------------------------------------------------
#
# On macOS point at the binary INSIDE the installed bundle so the process
# that actually runs carries the com.trycua.driver identity (TCC keys
# grants on it). On Linux/Windows point at the versioned-store binary.
mkdir -p "$BIN_DIR"
if [ "$OS" = "Darwin" ]; then
    BIN_TARGET="$APP_DEST/Contents/MacOS/cua-driver"
else
    BIN_TARGET="$CURRENT_LINK/cua-driver"
fi
ln -sf "$BIN_TARGET" "$BIN_DIR/cua-driver"
echo "${GREEN}$BIN_DIR/cua-driver -> $BIN_TARGET${NORMAL}"
echo ""

INSTALLED_BIN="$BIN_DIR/cua-driver"

# Install the skill pack through the same manifest validation and atomic
# activation path used by release and main installs. This explicit local
# source guarantees the installed instructions come from this checkout.
if [ ! -d "$SOURCE_SKILLS" ]; then
    echo "${RED}Error: local skill source is missing.${NORMAL}" >&2
    exit 1
fi
LOCAL_GIT_COMMIT="$(git -C "$REPO_ROOT" rev-parse HEAD 2>/dev/null || true)"
LOCAL_SKILL_ARGS=(skills update --from local --source "$SOURCE_SKILLS")
if [[ "$LOCAL_GIT_COMMIT" =~ ^[0-9a-fA-F]{40}$ ]]; then
    LOCAL_SKILL_ARGS+=(--git-commit "$LOCAL_GIT_COMMIT")
fi
CUA_DRIVER_RS_HOME="$HOME_DIR" "$INSTALLED_BIN" "${LOCAL_SKILL_ARGS[@]}"
echo "${GREEN}installed verified local skill pack${NORMAL}"

# --- Stop any pre-swap cua-driver daemons ------------------------------
#
# Mirror of install-local.ps1's daemon kill — the new binary is now
# under packages/current/, but any LaunchAgent / systemd user unit /
# manual `serve` shell is still running off the OLD binary. Stop them
# so the next invocation picks up this build. Best-effort, never
# fails the install. Survivors (rare on Unix — `pkill` reaches all
# user-owned procs without elevation) get a yellow hint.
stop_cua_driver_daemons
show_cua_driver_daemon_survivors

echo ""

# --- Autostart (optional) ----------------------------------------------

if [ "$INSTALL_AUTOSTART" = true ]; then
    if [ "$OS" = "Darwin" ]; then
        # Unload + remove any stale LaunchAgent from the pre-rename install
        # so the new label doesn't race the old one.
        LEGACY_PLIST_PATH="$HOME/Library/LaunchAgents/com.trycua.cua-driver-rs.plist"
        if [ -f "$LEGACY_PLIST_PATH" ]; then
            launchctl unload "$LEGACY_PLIST_PATH" 2>/dev/null || true
            rm -f "$LEGACY_PLIST_PATH"
        fi
        PLIST_PATH="$HOME/Library/LaunchAgents/com.trycua.cua-driver.plist"
        echo "${BOLD}Writing LaunchAgent → $PLIST_PATH${NORMAL}"
        mkdir -p "$(dirname "$PLIST_PATH")"
        cat >"$PLIST_PATH" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.trycua.cua-driver</string>
  <key>ProgramArguments</key>
  <array>
    <string>$INSTALLED_BIN</string>
    <string>serve</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>$HOME_DIR/serve.out.log</string>
  <key>StandardErrorPath</key><string>$HOME_DIR/serve.err.log</string>
</dict>
</plist>
EOF
        launchctl unload "$PLIST_PATH" 2>/dev/null || true
        launchctl load "$PLIST_PATH"
        echo "${GREEN}Loaded.${NORMAL} Manage with launchctl load / unload \"$PLIST_PATH\"."
    elif [ "$OS" = "Linux" ]; then
        # Stop + remove any pre-rename systemd unit so the new one doesn't
        # race the old one.
        LEGACY_UNIT_PATH="$HOME/.config/systemd/user/cua-driver-rs.service"
        if [ -f "$LEGACY_UNIT_PATH" ]; then
            systemctl --user disable --now cua-driver-rs.service 2>/dev/null || true
            rm -f "$LEGACY_UNIT_PATH"
        fi
        UNIT_PATH="$HOME/.config/systemd/user/cua-driver.service"
        echo "${BOLD}Writing systemd user unit → $UNIT_PATH${NORMAL}"
        mkdir -p "$(dirname "$UNIT_PATH")"
        cat >"$UNIT_PATH" <<EOF
[Unit]
Description=cua-driver serve daemon
After=graphical-session.target

[Service]
ExecStart=$INSTALLED_BIN serve
Restart=on-failure
RestartSec=2

[Install]
WantedBy=default.target
EOF
        systemctl --user daemon-reload
        systemctl --user enable --now cua-driver.service
        echo "${GREEN}Enabled.${NORMAL} Manage with systemctl --user {start|stop|status} cua-driver."
    fi
    echo ""
fi

# --- Done ---------------------------------------------------------------

echo "${BOLD}${GREEN}Installed.${NORMAL}"
echo "  ${BOLD}$INSTALLED_BIN${NORMAL}"
echo ""

# Unified post-install hints come from a single shared text file so the
# 4 Rust installers (this script + install-local.ps1 + _install-rust.sh +
# install.ps1) never drift. The .txt holds the OS-agnostic bulk
# (Try-it / skill pack / MCP setup / docs link) with {{BINARY}}
# placeholders; OS-specific bits stay inline below.
HINTS_TXT="$SCRIPT_DIR/post-install-hints.txt"
if [ -f "$HINTS_TXT" ]; then
    sed "s|{{BINARY}}|$INSTALLED_BIN|g" "$HINTS_TXT"
else
    # Repo layout changed or running from an unexpected location — fall
    # back to one-line essentials so users still know what to do next.
    echo "Next steps: $INSTALLED_BIN --version  |  $INSTALLED_BIN mcp-config  |  $INSTALLED_BIN skills install"
    echo "Docs: https://github.com/trycua/cua/tree/main/libs/cua-driver/rust"
fi

# OS-specific autostart hint (kept inline; per-shell natural location).
if [ "$INSTALL_AUTOSTART" != true ]; then
    echo ""
    if [ "$OS" = "Darwin" ]; then
        echo "Auto-start (recommended on macOS): re-run with --autostart to register a LaunchAgent."
        echo "  A launchd-started daemon is attributed to com.trycua.driver (not your terminal),"
        echo "  so permission prompts say \"Cua Driver\" and grants stick — grant Accessibility +"
        echo "  Screen Recording once and every cua-driver call/mcp routes through it correctly."
        echo "  (Without it, a prompt raised from a terminal attributes to the terminal instead.)"
    else
        echo "Auto-start (optional): re-run with --autostart to register a systemd user unit."
    fi
    echo ""
fi
