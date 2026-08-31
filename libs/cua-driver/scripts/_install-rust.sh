#!/usr/bin/env bash
# _install-rust.sh — private helper invoked by libs/cua-driver/scripts/install.sh
# (the canonical user-facing installer) for the default Rust implementation.
# Not intended for direct
# invocation; user-facing one-liners always go through the parent
# install.sh, which forwards args + sets up the lockfile.
#
# Downloads the latest cua-driver-rs release tarball from GitHub Releases
# and drops the binary into ~/.local/bin (or a path given via --bin-dir /
# CUA_DRIVER_RS_INSTALL_DIR). Sudo-free.
#
# This is the cross-platform cua-driver implementation for macOS / Linux /
# Windows via WSL or git-bash. The retired Swift implementation (macOS
# only) still ships separately under tag prefix `cua-driver-v*`; this
# helper is hard-pinned to `cua-driver-rs-v*` and will never pick it up.
#
# Canonical user-facing invocation (forwards here by default):
#   /bin/bash -c "$(curl -fsSL https://cua.ai/driver/install.sh)"
#
# Flags:
#   --bin-dir <path>     install the visible binary/symlink to <path>
#                        instead of ~/.local/bin
#   --no-modify-path     skip auto-appending an `export PATH=...` line
#   --channel <name>     persist and install the latest stable or nightly release
#
# Env overrides:
#   CUA_DRIVER_RS_VERSION=0.1.2          pin a stable release
#   CUA_DRIVER_RS_VERSION=nightly-cua-driver-rs-v0.1.3-nightly.20260812.42
#                                        pin an exact nightly release
#   CUA_DRIVER_RS_INSTALL_DIR=PATH       same as --bin-dir; sets the visible
#                                        binary location
#   CUA_DRIVER_RS_BIN_DIR=PATH           legacy alias for INSTALL_DIR
#   CUA_DRIVER_RS_HOME=PATH              package home for versioned installs
#                                        (default ~/.cua-driver). Holds
#                                        packages/releases/<v>-<target>/ and
#                                        packages/current/ on Linux/Windows.
#                                        Renamed from ~/.cua-driver-rs in
#                                        v0.2.16 / PR #1644 — this release
#                                        installer was missed in that rename
#                                        and is reconciled here; a stale
#                                        ~/.cua-driver-rs is swept post-install.
#   CUA_DRIVER_RS_NO_MODIFY_PATH=1       same as --no-modify-path
#   CUA_DRIVER_RS_KEEP_VERSIONS=N        keep the N most recent per-version
#                                        release dirs after install; older
#                                        ones are deleted (default 5; set 0
#                                        to disable GC entirely). Per-target
#                                        — multi-arch dirs are pruned
#                                        independently of each other.
#
# On-disk layout (Linux; macOS keeps its .app-in-/Applications layout, see
# below):
#   $CUA_DRIVER_RS_HOME/
#     packages/
#       releases/
#         0.1.3-x86_64-unknown-linux-gnu/cua-driver   (per-version binary)
#         0.1.4-x86_64-unknown-linux-gnu/cua-driver
#       current/cua-driver -> ../releases/<active>/cua-driver  (active version)
#   $CUA_DRIVER_RS_INSTALL_DIR/cua-driver -> $HOME/packages/current/cua-driver
#
# Atomic upgrade: a new install drops the binary into a fresh per-version
# dir, then rename(2)-swaps the `current` symlink to point at it. A
# running daemon keeps its already-mmap'd binary open across the swap
# (open file handles survive). Rollback: re-point `current` at any older
# entry under `releases/`.
#
# Post-install GC trims the per-target release dirs so disk usage stays
# bounded (each release dir is ~15 MB, so an indefinite series of
# upgrades grows without bound). The N most-recent dirs are kept (default
# 5, override via CUA_DRIVER_RS_KEEP_VERSIONS); the dir that `current`
# resolves to is always preserved even if its mtime would otherwise drop
# it off the list. GC runs after the atomic swap so the about-to-be-active
# version is never a deletion candidate.
#
set -euo pipefail

# --- Load shared daemon-cleanup helpers ---------------------------------
#
# Bash counterpart of install.ps1's `Import-CuaDriverInstallModule`. On
# a checked-out tree (`$BASH_SOURCE` points at a real file) we source
# the sibling _install-common.sh from disk. When this script is run via
# `curl ... | bash` (the production path forwarded from install.sh on
# Linux), there's no on-disk copy — fall back to curling the canonical
# raw URL and sourcing the downloaded tempfile.
#
# Failure here is non-fatal: the daemon-stop is a best-effort upgrade
# nicety, not load-bearing. If we can't load the helpers, define
# no-op stubs so the rest of the script can call them unconditionally.
_CUA_INSTALL_COMMON_URL="https://cua.ai/driver/_install-common.sh"
_cua_install_common_loaded=0
if [[ -n "${BASH_SOURCE[0]:-}" && "${BASH_SOURCE[0]}" != "-" && -f "${BASH_SOURCE[0]}" ]]; then
    _CUA_SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
    if [[ -f "$_CUA_SCRIPT_DIR/_install-common.sh" ]]; then
        # shellcheck source=_install-common.sh
        . "$_CUA_SCRIPT_DIR/_install-common.sh" && _cua_install_common_loaded=1
    fi
fi
if [[ "$_cua_install_common_loaded" == "0" ]] && command -v curl >/dev/null 2>&1; then
    _cua_install_common_tmp="$(mktemp -t cua-install-common.XXXXXX 2>/dev/null || mktemp)"
    if curl -fsSL "$_CUA_INSTALL_COMMON_URL" -o "$_cua_install_common_tmp" 2>/dev/null; then
        # shellcheck source=/dev/null
        . "$_cua_install_common_tmp" && _cua_install_common_loaded=1
    fi
    rm -f "$_cua_install_common_tmp" 2>/dev/null || true
fi
if [[ "$_cua_install_common_loaded" == "0" ]]; then
    # Stubs so call sites stay unconditional. Print a one-line warning
    # so a fetch failure shows up in the install log, but don't fail
    # the install over it — the binary swap below is the load-bearing
    # part, daemon cleanup is "nice to have".
    printf 'warning: could not load _install-common.sh (on-disk + network); daemon kill skipped\n' >&2
    stop_cua_driver_daemons() { :; }
    show_cua_driver_daemon_survivors() { :; }
fi

REPO="trycua/cua"
BINARY_NAME="cua-driver"
TAG_PREFIX="cua-driver-rs-v"
NIGHTLY_TAG_PREFIX="nightly-cua-driver-rs-v"
# CUA_DRIVER_RS_INSTALL_DIR is the documented name; CUA_DRIVER_RS_BIN_DIR is
# the legacy alias kept for users with the old env in their shell rc.
BIN_DIR="${CUA_DRIVER_RS_INSTALL_DIR:-${CUA_DRIVER_RS_BIN_DIR:-$HOME/.local/bin}}"
# Canonical home is ~/.cua-driver (renamed from ~/.cua-driver-rs in v0.2.16 /
# PR #1644). The local installer (_install-local-rust.sh) and the runtime
# already default here; this release installer was missed in that rename and
# kept writing to the legacy ~/.cua-driver-rs, which is the root cause of the
# install collision (release wrote one home, install-local + runtime used the
# other). Reconcile the default here, keep accepting the CUA_DRIVER_RS_HOME
# override for back-compat, and sweep the stale legacy dir post-install below.
HOME_DIR="${CUA_DRIVER_RS_HOME:-$HOME/.cua-driver}"
# Pre-v0.2.16 home this installer used to write to. Swept after the new
# install is staged so a single rooted home (~/.cua-driver) is left behind.
LEGACY_HOME_DIR="$HOME/.cua-driver-rs"
NO_MODIFY_PATH="${CUA_DRIVER_RS_NO_MODIFY_PATH:-0}"
# Post-install GC: how many per-version release dirs to retain. Validated
# below as a non-negative integer; 0 means "never GC". The dir that
# `current` resolves to is always preserved regardless of cutoff.
KEEP_VERSIONS_DEFAULT=5
KEEP_VERSIONS="${CUA_DRIVER_RS_KEEP_VERSIONS:-$KEEP_VERSIONS_DEFAULT}"
CHANNEL_ARG=""
CHANNEL_EXPLICIT=0

# macOS-only: name and install location of the .app bundle that wraps
# the bare binary so the TCC auto-relaunch path in `cua-driver mcp` has
# a stable bundle id (com.trycua.driver) to attribute the daemon to.
# See libs/cua-driver/rust/scripts/CuaDriverBundle/Contents/Info.plist and
# the matching docs on `cua-driver mcp`'s auto-relaunch behavior.
# Identical to the Swift driver's CuaDriver.app + com.trycua.driver
# pair — the Rust port replaces the Swift install at this path,
# preserving TCC grants (they're keyed on bundle id, which we share).
APP_NAME="CuaDriver.app"
APP_DEST="/Applications/$APP_NAME"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --bin-dir) BIN_DIR="$2"; shift 2 ;;
        --bin-dir=*) BIN_DIR="${1#*=}"; shift ;;
        --no-modify-path) NO_MODIFY_PATH=1; shift ;;
        --channel)
            [[ -n "${2:-}" ]] || { printf 'error: --channel requires stable or nightly\n' >&2; exit 2; }
            CHANNEL_ARG="$2"; CHANNEL_EXPLICIT=1; shift 2 ;;
        --channel=*) CHANNEL_ARG="${1#*=}"; CHANNEL_EXPLICIT=1; shift ;;
        *) shift ;;
    esac
done

# Validate KEEP_VERSIONS up front so a typo (e.g. "five") falls back to
# the default instead of silently disabling GC. Accepts any non-negative
# integer; 0 is the documented "never GC" sentinel.
if ! [[ "$KEEP_VERSIONS" =~ ^[0-9]+$ ]]; then
    printf 'warning: CUA_DRIVER_RS_KEEP_VERSIONS=%s is not a non-negative integer; falling back to %d\n' \
        "$KEEP_VERSIONS" "$KEEP_VERSIONS_DEFAULT" >&2
    KEEP_VERSIONS="$KEEP_VERSIONS_DEFAULT"
fi

BIN_LINK="$BIN_DIR/$BINARY_NAME"
TMP_DIR=$(mktemp -d)

log() { printf '==> %s\n' "$*"; }
err() { printf 'error: %s\n' "$*" >&2; }

# Return the source form of an app's designated code-signing requirement.
macos_designated_requirement() {
    codesign -d -r- "$1" 2>/dev/null \
        | sed -n -e 's/^designated => //p' -e 's/^# designated => //p'
}

# TCC stores the previous app's requirement, not just its bundle identifier.
# Ask Security.framework (through codesign) whether the replacement satisfies
# that exact requirement instead of comparing requirement text or cdhashes.
macos_requirement_compatibility() {
    local previous_requirement="$1"
    local candidate_app="$2"
    local codesign_output
    local codesign_status

    if [[ -z "$previous_requirement" ]]; then
        printf '%s' "unknown"
    elif codesign_output="$(codesign --verify --deep --strict \
            -R "=$previous_requirement" "$candidate_app" 2>&1)"; then
        printf '%s' "compatible"
    else
        codesign_status=$?
        if [[ "$codesign_status" == "3" ]]; then
            printf '%s' "incompatible"
        else
            printf 'warning: could not evaluate the previous code-signing requirement (codesign status %s); preserving TCC rows\n' \
                "$codesign_status" >&2
            [[ -z "$codesign_output" ]] \
                || printf 'warning: codesign: %s\n' "$codesign_output" >&2
            printf '%s' "unknown"
        fi
    fi
}

# Reset only the permissions Cua Driver itself consumes, and only after the
# newly installed bundle has been verified and registered with LaunchServices.
macos_reset_tcc_after_requirement_change() {
    local compatibility="$1"
    local bundle_id="com.trycua.driver"
    local failed_services=""
    local service

    [[ "$compatibility" == "incompatible" ]] || return 0
    if ! command -v tccutil >/dev/null 2>&1; then
        err "tccutil is required to clear stale Cua Driver permission rows after its signing requirement changed"
        return 1
    fi
    for service in Accessibility ScreenCapture; do
        if ! tccutil reset "$service" "$bundle_id" >/dev/null 2>&1; then
            failed_services="$failed_services $service"
        fi
    done
    if [[ -n "$failed_services" ]]; then
        err "could not reset these TCC services for $bundle_id:$failed_services"
        err "the new app is installed, but stale permission rows may remain; run:"
        err "  tccutil reset Accessibility $bundle_id"
        err "  tccutil reset ScreenCapture $bundle_id"
        return 1
    fi

    log "the app signing requirement changed; cleared stale Accessibility and Screen Recording rows"
    log "macOS authorization is required again: cua-driver permissions grant"
}

# --- Concurrent-install lockfile ---------------------------------------
#
# A second install kicked off while a first is still running can race
# on the atomic `current` symlink swap and produce a half-installed
# state (e.g. the symlink points at a dir whose binary the first install
# hasn't finished copying). Serialize installs per $HOME_DIR with a
# process-level mutex.
#
# Primitive: mkdir on POSIX is atomic. The first install to create
# $LOCK_DIR holds the lock; concurrent attempts get EEXIST and poll.
# Released via trap on every exit path (success, error, signal) so
# a half-finished install always frees the lock for the next one.
#
# Stale-lock recovery: if the holder dies without releasing (kill -9,
# OOM, host reboot mid-install), the lock dir sits around forever and
# every subsequent install hangs. After $LOCK_STALE_AFTER_SECONDS of
# waiting we probe the holder's liveness via `kill -0 <pid>` (the pid
# is stamped into $LOCK_INFO right after acquisition) — if the holder
# is alive we keep waiting (slow download / wedged network is not the
# same as a crashed install and we must not yank the lock out from
# under a live process), if it's dead we force-release with a loud log
# and proceed. The alternative (hang forever) leaves users in a
# permanently wedged state with no clear recovery path beyond `rm -rf`
# on an internal-looking dir.
LOCK_PACKAGES_DIR="$HOME_DIR/packages"
LOCK_DIR="$LOCK_PACKAGES_DIR/.install.lock.d"
LOCK_INFO="$LOCK_DIR/info"
LOCK_POLL_INTERVAL_SECONDS=1
LOCK_STALE_AFTER_SECONDS=600

LOCK_HELD=0
MACOS_APP_SWAP_STARTED=0
MACOS_APP_HAD_PREVIOUS=0
MACOS_APP_INSTALL_COMMITTED=0
MACOS_APP_BACKUP=""

restore_macos_app_backup_on_exit() {
    [[ "$MACOS_APP_SWAP_STARTED" == "1" ]] || return 0

    if [[ "$MACOS_APP_INSTALL_COMMITTED" == "1" ]]; then
        if [[ -e "$MACOS_APP_BACKUP" ]] && ! rm -rf "$MACOS_APP_BACKUP"; then
            printf 'warning: could not remove macOS install backup at %s\n' \
                "$MACOS_APP_BACKUP" >&2
        fi
        return 0
    fi

    if [[ "$MACOS_APP_HAD_PREVIOUS" == "1" ]]; then
        # If the backup does not exist, the atomic move never completed or an
        # explicit rollback already restored it. Leave the live path alone.
        if [[ -e "$MACOS_APP_BACKUP" ]]; then
            rm -rf "$APP_DEST"
            mv "$MACOS_APP_BACKUP" "$APP_DEST"
            printf 'warning: interrupted macOS install restored the previous CuaDriver.app\n' >&2
        fi
    else
        # A first install has no app to restore; remove only its partial copy.
        rm -rf "$APP_DEST"
    fi
}

release_install_lock() {
    if (( LOCK_HELD == 1 )); then
        rm -rf "$LOCK_DIR" 2>/dev/null || true
        LOCK_HELD=0
    fi
}

# Combine TMP_DIR cleanup with lock release in a single trap so neither
# clobbers the other. INT/TERM also re-raise via $? so the user-visible
# exit code reflects the signal.
cleanup_on_exit() {
    restore_macos_app_backup_on_exit
    rm -rf "$TMP_DIR" 2>/dev/null || true
    release_install_lock
}
trap cleanup_on_exit EXIT
trap 'cleanup_on_exit; trap - INT;  kill -INT  $$' INT
trap 'cleanup_on_exit; trap - TERM; kill -TERM $$' TERM

acquire_install_lock() {
    mkdir -p "$LOCK_PACKAGES_DIR"
    local waited=0
    while ! mkdir "$LOCK_DIR" 2>/dev/null; do
        if (( waited == 0 )); then
            log "another cua-driver-rs install is already in progress (lock at $LOCK_DIR); waiting..."
        fi
        sleep "$LOCK_POLL_INTERVAL_SECONDS"
        waited=$((waited + LOCK_POLL_INTERVAL_SECONDS))
        if (( waited >= LOCK_STALE_AFTER_SECONDS )); then
            # Don't yank the lock from a live install. Parse pid= from
            # the info file (written by the holder right after mkdir);
            # if that pid is still alive per kill -0, the holder is just
            # slow (big download, wedged network) — keep waiting. Only
            # reclaim when there's no live holder.
            #
            # Missing/unreadable info file → holder didn't get far enough
            # to stamp pid, so assume dead and reclaim. Unparseable pid
            # line → same. Either way we err on the side of progress
            # rather than hanging forever once the 600s window elapses.
            local holder_pid=""
            if [[ -r "$LOCK_INFO" ]]; then
                holder_pid=$(grep -E '^pid=' "$LOCK_INFO" 2>/dev/null | head -1 | cut -d= -f2)
            fi
            if [[ -n "$holder_pid" ]] && kill -0 "$holder_pid" 2>/dev/null; then
                log "lock at $LOCK_DIR still held by live pid $holder_pid; continuing to wait"
                # Reset waited so we re-check after another full window
                # rather than spamming this branch every poll interval.
                waited=0
                continue
            fi
            log "lock at $LOCK_DIR appears stale (>${LOCK_STALE_AFTER_SECONDS}s, no live holder); forcing release"
            rm -rf "$LOCK_DIR" 2>/dev/null || true
            waited=0
        fi
    done
    LOCK_HELD=1
    # Drop pid + ISO timestamp + invocation args into the lock dir so a
    # user investigating a stuck install can see who holds it (`cat
    # $HOME_DIR/packages/.install.lock.d/info`).
    {
        printf 'pid=%s\n' "$$"
        printf 'started=%s\n' "$(date -u +'%Y-%m-%dT%H:%M:%SZ' 2>/dev/null || date)"
        printf 'argv=%s\n' "$0 $*"
    } > "$LOCK_INFO" 2>/dev/null || true
}

acquire_install_lock "$@"

# Prune per-version release dirs under $RELEASES_DIR for the current
# $TARGET, keeping the N most recent (by mtime). The dir that the
# `current` symlink resolves to is always preserved — even if it's
# older than the cutoff — so we never delete the active install.
#
# Filtering is by target-triple suffix so a multi-arch dev with both
# (e.g.) aarch64-apple-darwin and x86_64-unknown-linux-gnu under the
# same $HOME_DIR keeps each target's history independently. Other-arch
# dirs are invisible to this prune pass.
#
# Args: $1 = releases dir, $2 = current symlink path, $3 = target triple,
#       $4 = keep count (non-negative integer; 0 = skip).
prune_old_releases() {
    local releases_dir="$1" current_link="$2" target="$3" keep="$4"

    if [[ "$keep" == "0" ]]; then
        log "version GC disabled (CUA_DRIVER_RS_KEEP_VERSIONS=0)"
        return 0
    fi
    if [[ ! -d "$releases_dir" ]]; then
        return 0
    fi

    # Resolve the dir `current` points at so we can exempt it from the
    # prune candidates. `readlink -f` would canonicalize, but BSD readlink
    # (macOS — which doesn't reach here in production but is tested from
    # a dev shell) lacks -f, so use the more portable two-step.
    local current_target=""
    if [[ -L "$current_link" ]]; then
        local link_value
        link_value=$(readlink "$current_link" 2>/dev/null || true)
        if [[ -n "$link_value" ]]; then
            case "$link_value" in
                /*) current_target="$link_value" ;;
                *)  current_target="$(dirname "$current_link")/$link_value" ;;
            esac
        fi
    fi

    # `ls -dt` sorts dirs by mtime, newest-first. The trailing slash on
    # the glob filters to dirs only. Skip if no matching dirs (the glob
    # would otherwise pass through literally under shopt -s nullglob being
    # off — guard with 2>/dev/null and an `|| true`).
    local candidates=()
    while IFS= read -r dir; do
        [[ -n "$dir" ]] || continue
        # Strip trailing slash for consistent comparison with $current_target.
        dir="${dir%/}"
        # Only consider dirs whose name ends in the current target triple.
        local base="${dir##*/}"
        case "$base" in
            *-"$target") candidates+=("$dir") ;;
            *) ;;
        esac
    done < <(ls -dt "$releases_dir"/*/ 2>/dev/null || true)

    if [[ ${#candidates[@]} -le "$keep" ]]; then
        return 0
    fi

    # Walk candidates newest-first. The first $keep are retained by mtime
    # (active install counts toward the budget in the common case where
    # the install just happened — it's the newest by mtime). The active
    # install is *additionally* preserved even if it would otherwise fall
    # outside the keep window (e.g. user rolled back to an old version),
    # so the worst-case post-GC count is $keep + 1, common case is exactly
    # $keep.
    local to_prune=()
    local kept=0
    local i
    for ((i=0; i<${#candidates[@]}; i++)); do
        local cand="${candidates[$i]}"
        local is_current=0
        if [[ -n "$current_target" && "${cand%/}" == "${current_target%/}" ]]; then
            is_current=1
        fi
        if (( kept < keep )); then
            kept=$((kept + 1))
            continue
        fi
        if (( is_current == 1 )); then
            # Active install fell outside the keep window — preserve
            # anyway (never delete the dir that's about to serve the
            # next `cua-driver` invocation).
            continue
        fi
        to_prune+=("$cand")
    done

    if [[ ${#to_prune[@]} -eq 0 ]]; then
        return 0
    fi

    log "pruning ${#to_prune[@]} old release dir(s) (keeping $keep most recent for $target):"
    local p
    for p in "${to_prune[@]}"; do
        log "  - ${p##*/}"
    done
    printf '%s\0' "${to_prune[@]}" | xargs -0 rm -rf
}

# --- Clean up a pre-existing LOCAL (install-local) install --------------
#
# `install-local.sh` (`_install-local-rust.sh`) installs a dev build into the
# SAME canonical home this release installer now writes to (~/.cua-driver, see
# the HOME_DIR reconciliation above), under a `*-local-*` versioned release dir
# (VERSION_TAG="0.0.0-local-<config>"). On macOS it also cert-signs the shared
# /Applications/CuaDriver.app with a self-signed identity recorded at
# `~/.cua-driver/.tcc-signing-identity`.
#
# A user who ran install-local and then runs this release installer would
# otherwise end up with the local artifacts lingering alongside the fresh
# release: the `*-local-*` release dir(s) sit in `packages/releases/` (the
# release `current` swap re-points away from them, but they're never removed
# explicitly here), and the stale `.tcc-signing-identity` marker survives even
# though the release bundle is CI-signed, not locally cert-signed. Follow the
# same logic install-local / uninstall.sh use: stop the daemon, then remove
# ONLY the unambiguously-local artifacts so the release install is the single
# authoritative one.
#
# Conservative by construction: we only ever remove `*-local-*` release dirs
# and the local signing-identity marker — never a real release dir, never the
# `current` symlink (the release branch owns that), never unrelated user state
# under the home. Every step is best-effort + idempotent; a machine with no
# prior local install is a clean no-op.
#
# The release install below verifies the previous and replacement designated
# requirements. Compatible releases preserve grants; a proven mismatch resets
# only the stale Cua Driver rows after the replacement is registered.
cleanup_prior_local_install() {
    local releases_dir="$HOME_DIR/packages/releases"
    local tcc_marker="$HOME_DIR/.tcc-signing-identity"

    # Collect the local-build release dirs (the unambiguous install-local
    # signature — a release install never creates a `*-local-*` dir).
    local local_dirs=()
    local d
    if [[ -d "$releases_dir" ]]; then
        for d in "$releases_dir"/*-local-*/; do
            [[ -d "$d" ]] && local_dirs+=("${d%/}")
        done
    fi

    # Nothing local on disk → clean no-op (no marker, no local dirs).
    if [[ ${#local_dirs[@]} -eq 0 && ! -f "$tcc_marker" ]]; then
        return 0
    fi

    log "detected a prior install-local build under $HOME_DIR — cleaning it up so this release install is authoritative"

    # Stop the local daemon BEFORE we yank its binary out from under it,
    # mirroring the post-swap stop both installers already do. Best-effort.
    stop_cua_driver_daemons

    # Remove the `*-local-*` release dirs. The release install stages into its
    # own `<version>-<target>` dir and swaps `current` to it, so deleting the
    # local dirs can't strand the active install. If `current` somehow still
    # points into a local dir (e.g. a partial prior run), the release branch
    # below re-creates `current` immediately after, so a transient dangling
    # link is harmless.
    if [[ ${#local_dirs[@]} -gt 0 ]]; then
        for d in "${local_dirs[@]}"; do
            rm -rf "$d" 2>/dev/null || true
            log "  removed local build dir ${d##*/}"
        done
    fi

    # Remove the local signing-identity marker — it describes the locally
    # cert-signed bundle, which the release `ditto` is about to replace with
    # the CI-signed one. Leaving it would misreport the bundle's identity.
    if [[ -f "$tcc_marker" ]]; then
        rm -f "$tcc_marker" 2>/dev/null || true
        log "  removed local signing-identity marker $tcc_marker"
    fi
}

# --- Resolve OS/arch ----------------------------------------------------

OS=$(uname -s)
ARCH_RAW=$(uname -m)

# Rosetta translation correction.
#
# `uname -m` reflects the architecture of the *running process*, not the
# physical CPU. When an Apple Silicon Mac is driving an x86_64-translated
# shell (Rosetta — e.g. `arch -x86_64 bash`, or a Homebrew install pinned
# to /usr/local/), uname reports x86_64 even though the native arch is
# arm64. We'd then download the x86_64 binary and run it under Rosetta —
# slower, and an unnecessary translation when a native arm64 binary
# exists on the release page.
#
# `sysctl.proc_translated` returns 1 when the current process is running
# under Rosetta translation; absent/0 means native. Only meaningful on
# macOS — the sysctl key is missing on Linux, so the redirect-to-null
# keeps the check a silent no-op there.
if [[ "$OS" == "Darwin" && "$ARCH_RAW" == "x86_64" ]]; then
    if [[ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" == "1" ]]; then
        log "detected Rosetta-translated shell on Apple Silicon — switching to darwin-arm64"
        ARCH_RAW="arm64"
    fi
fi

# LABEL  = the release-asset tarball label (matches what cd-rust-cua-driver.yml
#          publishes; user-facing).
# TARGET = the Rust target triple, used in the on-disk per-version dir name so
#          a multi-arch dev can keep e.g. aarch64-apple-darwin and
#          x86_64-unknown-linux-gnu side by side under $HOME_DIR/packages/
#          releases/ without collision.
case "$OS-$ARCH_RAW" in
    Darwin-arm64|Darwin-aarch64)     LABEL="darwin-arm64"  ; TARGET="aarch64-apple-darwin"      ;;
    Darwin-x86_64)                   LABEL="darwin-x86_64" ; TARGET="x86_64-apple-darwin"       ;;
    Linux-x86_64|Linux-amd64)        LABEL="linux-x86_64"  ; TARGET="x86_64-unknown-linux-gnu"  ;;
    Linux-aarch64|Linux-arm64)       LABEL="linux-arm64"   ; TARGET="aarch64-unknown-linux-gnu" ;;
    *)
        err "unsupported platform: $OS / $ARCH_RAW"
        err "  cua-driver-rs ships prebuilts for: darwin-arm64, darwin-x86_64, linux-x86_64, linux-arm64."
        err "  Windows users: install via install.ps1 (irm https://cua.ai/driver/install.ps1 | iex)."
        exit 1
        ;;
esac

for cmd in curl tar; do
    if ! command -v "$cmd" >/dev/null 2>&1; then
        err "$cmd not found on PATH"
        exit 1
    fi
done

# --- Resolve release tag ------------------------------------------------
#
# Version is resolved in priority order:
#   1. CUA_DRIVER_RS_VERSION env var (explicit pin)
#   2. CUA_DRIVER_RS_BAKED_VERSION below (updated after release publication)
#   3. GitHub Releases API (fallback for dev / un-baked checkouts;
#      unauthenticated = 60 req/hr per IP)
#
# The baked value is the common-case default: `curl ... | bash` against
# `main` resolves the version locally with zero API calls, so an API
# outage / rate limit / network blip can't break a default install. The
# API is consulted only when the baked line is absent (dev / pre-release
# checkouts) or when the baked version turns out to have no downloadable
# asset — see the recovery at the download step below.
#
# ~~~ BAKED_VERSION: auto-updated after release publication — do not edit ~~~
CUA_DRIVER_RS_BAKED_VERSION="0.23.2" # published-installer-version
# ~~~ END_BAKED_VERSION ~~~

# Run API requests with an optional token. Keep the header construction here
# (rather than in loggable command text) so neither GH_TOKEN nor GITHUB_TOKEN
# can appear in installer output. Release-asset downloads stay unauthenticated:
# the repository is public and curl may redirect them to another GitHub host.
# GH_TOKEN takes precedence, matching the GitHub CLI.
github_api_curl() {
    local token="${GH_TOKEN:-${GITHUB_TOKEN:-}}"
    if [[ -n "$token" ]]; then
        curl -H "Authorization: Bearer $token" "$@"
    else
        curl "$@"
    fi
}

# The authenticated releases endpoint can include drafts for maintainers.
# Associate each top-level tag_name with its following draft field before
# considering it. GitHub's REST response renders those top-level fields on
# separate lines in that order; nested author/assets objects have no tag_name.
extract_published_release_versions() {
    # Keep this helper usable by stable-only callers and extracted test
    # harnesses that predate persistent channel selection.
    local selected_channel="${SELECTED_CHANNEL:-stable}"
    local selected_tag_prefix="${SELECTED_TAG_PREFIX:-$TAG_PREFIX}"
    awk -v prefix="$selected_tag_prefix" -v channel="$selected_channel" '
        /"tag_name"[[:space:]]*:/ {
            tag = $0
            sub(/^.*"tag_name"[[:space:]]*:[[:space:]]*"/, "", tag)
            sub(/".*$/, "", tag)
            next
        }
        tag != "" && /"draft"[[:space:]]*:/ {
            if ($0 ~ /"draft"[[:space:]]*:[[:space:]]*false/) {
                version = tag
                if (index(version, prefix) == 1) {
                    version = substr(version, length(prefix) + 1)
                    if ((channel == "stable" && version ~ /^[0-9]+\.[0-9]+\.[0-9]+$/) ||
                        (channel == "nightly" && version ~ /^[0-9]+\.[0-9]+\.[0-9]+-nightly\.[0-9]{8}\.[1-9][0-9]*$/)) {
                        print version
                    }
                }
            }
            tag = ""
        }
    '
}

# Highest SemVer ${TAG_PREFIX}* version published on the repo, printed bare
# (no tag prefix) on stdout. Returns non-zero when the API is unreachable or
# has no matching tag; callers decide whether that is fatal, because this runs
# both as the primary resolver and as recovery for a bad baked version.
#
# per_page=100 (the API maximum): the repo interleaves lume, Python, and Swift
# releases with these. Walk up to ten pages so a busy repository cannot hide
# cua-driver-rs behind the first page, but keep the request count bounded.
resolve_latest_version_from_api() {
    local page page_json page_count page_versions
    local versions=""
    for ((page=1; page<=10; page++)); do
        page_json="$(github_api_curl -fsSL \
            "https://api.github.com/repos/$REPO/releases?per_page=100&page=$page")" || return 1

        # Extract only published tags matching the selected channel's strict
        # grammar. Cua Driver's stable tags are marked prerelease in GitHub
        # metadata, so tag syntax—not the prerelease flag—defines the channel.
        page_versions="$(printf '%s' "$page_json" | extract_published_release_versions)" || true
        if [[ -n "$page_versions" ]]; then
            versions="${versions}${versions:+$'\n'}${page_versions}"
        fi

        page_count="$(
            printf '%s' "$page_json" \
                | awk '{ count += gsub(/"tag_name"[[:space:]]*:/, "&") } END { print count + 0 }'
        )"
        [[ "$page_count" =~ ^[0-9]+$ ]] || return 1
        if (( page_count < 100 )); then
            break
        fi
    done

    local version
    version="$(
        printf '%s\n' "$versions" \
            | sed '/^$/d' \
            | sort -t. -k1,1nr -k2,2nr -k3,3nr -k4,4nr -k5,5nr \
            | head -n 1
    )"
    [[ -n "$version" ]] || return 1
    printf '%s' "$version"
}

# Where VERSION came from. A missing asset is fatal for an explicit pin (the
# user named that version) but recoverable for the baked constant. The normal
# CD path advances it only after every staged release asset is public; fallback
# remains defense in depth for manual edits, asset removal, or an interrupted
# legacy release flow.
resolve_explicit_release_tag() {
    local value="$1" stable_version nightly_version
    if [[ "$value" =~ ^(cua-driver-rs-v|v)?([0-9]+\.[0-9]+\.[0-9]+)$ ]]; then
        stable_version="${BASH_REMATCH[2]}"
        printf '%s%s' "$TAG_PREFIX" "$stable_version"
        return 0
    fi
    if [[ "$value" =~ ^nightly-cua-driver-rs-v([0-9]+\.[0-9]+\.[0-9]+-nightly\.[0-9]{8}\.[1-9][0-9]*)$ ]]; then
        nightly_version="${BASH_REMATCH[1]}"
        printf '%s%s' "$NIGHTLY_TAG_PREFIX" "$nightly_version"
        return 0
    fi
    if [[ "$value" =~ ^([0-9]+\.[0-9]+\.[0-9]+-nightly\.[0-9]{8}\.[1-9][0-9]*)$ ]]; then
        nightly_version="${BASH_REMATCH[1]}"
        printf '%s%s' "$NIGHTLY_TAG_PREFIX" "$nightly_version"
        return 0
    fi
    return 1
}

CHANNEL_STATE_FILE="$HOME_DIR/release-channel"
if [[ "$CHANNEL_EXPLICIT" == "1" && -n "${CUA_DRIVER_RS_VERSION:-}" ]]; then
    err "--channel cannot be combined with CUA_DRIVER_RS_VERSION; exact pins do not change saved channel state"
    exit 2
fi
if [[ "$CHANNEL_EXPLICIT" == "1" ]]; then
    SELECTED_CHANNEL="$CHANNEL_ARG"
elif [[ -n "${CUA_DRIVER_RS_VERSION:-}" ]]; then
    # Exact pins are one-shot and outrank persisted preference. In particular,
    # a damaged preference file must not make a deliberate recovery pin unusable.
    SELECTED_CHANNEL="stable"
elif [[ -f "$CHANNEL_STATE_FILE" ]]; then
    SELECTED_CHANNEL="$(tr -d '[:space:]' < "$CHANNEL_STATE_FILE")"
else
    SELECTED_CHANNEL="stable"
fi
case "$SELECTED_CHANNEL" in
    stable) SELECTED_TAG_PREFIX="$TAG_PREFIX" ;;
    nightly) SELECTED_TAG_PREFIX="$NIGHTLY_TAG_PREFIX" ;;
    *)
        err "invalid release channel '$SELECTED_CHANNEL' in $CHANNEL_STATE_FILE; expected stable or nightly"
        err "  repair with: cua-driver channel set stable"
        exit 1
        ;;
esac

if [[ -n "${CUA_DRIVER_RS_VERSION:-}" ]]; then
    VERSION_SOURCE="pin"
    if ! TAG="$(resolve_explicit_release_tag "$CUA_DRIVER_RS_VERSION")"; then
        err "CUA_DRIVER_RS_VERSION must be an exact x.y.z stable version or canonical nightly tag"
        exit 1
    fi
    log "using version from CUA_DRIVER_RS_VERSION: $TAG"
elif [[ "$SELECTED_CHANNEL" == "stable" && -n "${CUA_DRIVER_RS_BAKED_VERSION:-}" ]]; then
    VERSION_SOURCE="baked"
    if ! [[ "$CUA_DRIVER_RS_BAKED_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
        err "baked Cua Driver version must be an exact stable x.y.z version"
        exit 1
    fi
    TAG="${TAG_PREFIX}${CUA_DRIVER_RS_BAKED_VERSION#v}"
    log "using baked release: $TAG"
else
    VERSION_SOURCE="api"
    log "resolving latest $SELECTED_CHANNEL release via GitHub API"
    if ! API_VERSION="$(resolve_latest_version_from_api)"; then
        err "no release matching ${SELECTED_TAG_PREFIX}* found on $REPO"
        err "  (cua-driver-rs is a BETA-stage cross-platform port; releases may not be published yet.)"
        exit 1
    fi
    TAG="${SELECTED_TAG_PREFIX}${API_VERSION}"
    log "latest release: $TAG"
fi

if [[ "$TAG" == "$NIGHTLY_TAG_PREFIX"* ]]; then
    VERSION="${TAG#${NIGHTLY_TAG_PREFIX}}"
else
    VERSION="${TAG#${TAG_PREFIX}}"
fi

# Releases through 0.12.6 predate semantic cursor themes.
# Newer releases must contain both packaged copies.
CURSOR_THEME_REQUIRED_FROM="0.12.7"
version_is_at_least() {
    local version="$1" minimum="$2"
    local v_major v_minor v_patch m_major m_minor m_patch
    version="${version%%-*}"
    IFS=. read -r v_major v_minor v_patch <<< "$version"
    IFS=. read -r m_major m_minor m_patch <<< "$minimum"
    if (( v_major != m_major )); then (( v_major > m_major )); return; fi
    if (( v_minor != m_minor )); then (( v_minor > m_minor )); return; fi
    (( v_patch >= m_patch ))
}

# --- Download bare-binary tarball ---------------------------------------

# Tarball selection:
#
# macOS — fetch the directory tarball (cua-driver-rs-vN-darwin-universal.tar.gz).
#   The directory layout includes `CuaDriver.app/` alongside the bare
#   binary, which we need to install into /Applications so the TCC
#   auto-relaunch path in `cua-driver-rs mcp` can resolve
#   `com.trycua.driver` via `open -n -g -a CuaDriver`. The
#   directory variant carries the same universal binary as the
#   bare-binary tarball, so users on both Apple Silicon and Intel
#   get a working install from one download.
#
# Linux / Windows-via-WSL — use the bare-binary tarball. No bundle on
#   these platforms, no TCC, no need to unpack a directory.
release_tarball_name() {
    case "$LABEL" in
        darwin-*) printf 'cua-driver-rs-%s-darwin-universal.tar.gz' "$1" ;;
        *)        printf 'cua-driver-rs-%s-%s-binary.tar.gz' "$1" "$LABEL" ;;
    esac
}

# Fetches one release tarball into $TMP_DIR. A confirmed HTTP 404 returns 44;
# every other failure is retried at the same URL with bounded backoff, then
# returns 1. This distinction is load-bearing: only a missing baked asset may
# trigger release fallback. A timeout, TLS failure, rate limit, or server error
# must never silently install an older version.
download_release_tarball() {
    local version="$1" tarball url partial http_code curl_status attempt retryable
    tarball="$(release_tarball_name "$version")"
    url="https://github.com/$REPO/releases/download/${TAG}/$tarball"
    partial="$TMP_DIR/$tarball.partial"
    log "downloading $url"
    for attempt in 1 2 3; do
        http_code=""
        curl_status=0
        http_code="$(
            curl -sSL -o "$partial" -w '%{http_code}' "$url"
        )" || curl_status=$?
        if (( curl_status == 0 )) && [[ "$http_code" =~ ^2[0-9][0-9]$ ]]; then
            mv "$partial" "$TMP_DIR/$tarball"
            return 0
        fi
        rm -f "$partial" 2>/dev/null || true
        if [[ "$http_code" == "404" ]]; then
            return 44
        fi
        retryable=0
        if (( curl_status != 0 )) \
            || [[ "$http_code" == "408" || "$http_code" == "429" ]] \
            || [[ "$http_code" =~ ^5[0-9][0-9]$ ]]; then
            retryable=1
        fi
        if (( retryable == 1 && attempt < 3 )); then
            err "download attempt $attempt failed (HTTP ${http_code:-unknown}, curl exit $curl_status); retrying the same release"
            sleep "$attempt"
            continue
        fi
        break
    done
    err "download failed after $attempt attempt(s) (HTTP ${http_code:-unknown}, curl exit $curl_status); refusing to fall back to an older release"
    return 1
}

DOWNLOAD_STATUS=0
download_release_tarball "$VERSION" || DOWNLOAD_STATUS=$?
if (( DOWNLOAD_STATUS != 0 )); then
    # Defense in depth for a manually advanced constant, removed asset, or
    # interrupted legacy release flow. The normal CD path updates this constant
    # only after every staged asset is publicly visible.
    if [[ "$VERSION_SOURCE" != "baked" || "$DOWNLOAD_STATUS" != "44" ]]; then
        err "download failed; try CUA_DRIVER_RS_VERSION=<version> to pin a specific release"
        exit 1
    fi
    printf 'warning: baked release %s has no downloadable %s asset (HTTP 404); this is usually a temporary publish lag\n' \
        "$TAG" "$LABEL" >&2
    printf 'warning: temporarily falling back to the newest fully published release via the GitHub Releases API\n' >&2
    if ! API_VERSION="$(resolve_latest_version_from_api)"; then
        err "could not resolve any published ${TAG_PREFIX}* release to fall back to"
        err "  try CUA_DRIVER_RS_VERSION=<version> to pin a specific release"
        exit 1
    fi
    if [[ "$API_VERSION" == "$VERSION" ]]; then
        # The API agrees this is the newest tag, so the tag exists but its
        # assets do not. Retrying the identical URL would just 404 again.
        err "${TAG_PREFIX}${API_VERSION} is the newest published release but is missing its ${LABEL} asset"
        err "  try CUA_DRIVER_RS_VERSION=<version> to pin an older release"
        exit 1
    fi
    printf 'warning: falling back to %s%s\n' "$TAG_PREFIX" "$API_VERSION" >&2
    # Adopt the recovered release before anything downstream derives a path,
    # a stage directory, or a capability check from VERSION.
    VERSION="$API_VERSION"
    TAG="${TAG_PREFIX}${VERSION}"
    DOWNLOAD_STATUS=0
    download_release_tarball "$VERSION" || DOWNLOAD_STATUS=$?
    if (( DOWNLOAD_STATUS != 0 )); then
        err "download failed; try CUA_DRIVER_RS_VERSION=<version> to pin a specific release"
        exit 1
    fi
fi
TARBALL="$(release_tarball_name "$VERSION")"

log "extracting"
tar -xzf "$TMP_DIR/$TARBALL" -C "$TMP_DIR"

# Layout detection:
#   macOS dir tarball expands to:
#     cua-driver-rs-${VERSION}-darwin-universal/
#       ├── cua-driver           (bare universal binary)
#       ├── CuaDriver.app/     (minimal bundle; copy of the same binary
#       │                         lives at Contents/MacOS/cua-driver)
#       └── LICENSE
#   Linux bare-runtime tarball expands to:
#     cua-driver and libcua_driver_sdk.so at the archive root. The installer
#     consumes the CLI; SDK packaging consumes the colocated library.
case "$LABEL" in
    darwin-*)
        STAGE="cua-driver-rs-${VERSION}-darwin-universal"
        SRC="$TMP_DIR/$STAGE/$BINARY_NAME"
        SRC_THEME="$TMP_DIR/$STAGE/cua-cursor-theme"
        SRC_APP="$TMP_DIR/$STAGE/$APP_NAME"
        ;;
    *)
        SRC="$TMP_DIR/$BINARY_NAME"
        SRC_THEME="$TMP_DIR/cua-cursor-theme"
        SRC_WAYLAND_HELPER="$TMP_DIR/wayland-helper"
        SRC_APP=""
        ;;
esac
if [[ ! -f "$SRC" ]]; then
    err "expected $BINARY_NAME in tarball but didn't find it"
    ls -la "$TMP_DIR"
    exit 1
fi
THEME_AVAILABLE=1
if [[ ! -f "$SRC_THEME" ]] || {
    [[ -n "$SRC_APP" ]] &&
    [[ ! -f "$SRC_APP/Contents/MacOS/cua-cursor-theme" ]]
}; then
    THEME_AVAILABLE=0
fi
if [[ "$THEME_AVAILABLE" == "0" ]] && version_is_at_least \
    "$VERSION" "$CURSOR_THEME_REQUIRED_FROM"; then
    err "expected cua-cursor-theme in tarball but didn't find it"
    ls -la "$TMP_DIR"
    exit 1
fi
if [[ "$THEME_AVAILABLE" == "0" ]]; then
    printf 'warning: release %s predates cua-cursor-theme; installing without custom cursor themes\n' \
        "$VERSION" >&2
fi

if [[ "$CHANNEL_EXPLICIT" == "1" ]]; then
    mkdir -p "$HOME_DIR"
    CHANNEL_TMP="$HOME_DIR/.release-channel.$$"
    printf '%s\n' "$SELECTED_CHANNEL" > "$CHANNEL_TMP"
    mv -f "$CHANNEL_TMP" "$CHANNEL_STATE_FILE"
    log "saved release channel: $SELECTED_CHANNEL"
fi

# --- Install ------------------------------------------------------------

# Before staging the new release, sweep any prior install-local build that
# shares this home so the release install ends up authoritative (see the
# function definition above for the conservative marker-gated logic).
cleanup_prior_local_install

mkdir -p "$BIN_DIR"

# Persist the bounded installer channel before the new binary becomes visible.
# If the user invokes Cua Driver before the detached install-event hook wins
# the lifecycle lock, the ordinary first-run path will still use the installer
# attribution. The runtime removes this hint after lifecycle delivery succeeds.
INSTALL_CHANNEL="${CUA_DRIVER_INSTALL_CHANNEL:-install_script}"
case "$INSTALL_CHANNEL" in
    install_script|update_apply|python_package|first_run) ;;
    *) INSTALL_CHANNEL="install_script" ;;
esac

# Mirror the runtime's consent precedence before writing the attribution hint:
# environment override, compatibility override, persisted preference, default-on.
# This keeps an opted-out install free of telemetry state even when the detached
# install-event hook returns before reading the hint.
TELEMETRY_HINT_ENABLED=1
TELEMETRY_HINT_FROM_ENV=0
for telemetry_env_name in CUA_DRIVER_RS_TELEMETRY_ENABLED CUA_TELEMETRY_ENABLED; do
    telemetry_env_value="${!telemetry_env_name:-}"
    telemetry_env_value="$(printf '%s' "$telemetry_env_value" | tr '[:upper:]' '[:lower:]' | sed 's/^[[:space:]]*//; s/[[:space:]]*$//')"
    case "$telemetry_env_value" in
        1|true|yes|on)
            TELEMETRY_HINT_ENABLED=1
            TELEMETRY_HINT_FROM_ENV=1
            break
            ;;
        0|false|no|off)
            TELEMETRY_HINT_ENABLED=0
            TELEMETRY_HINT_FROM_ENV=1
            break
            ;;
    esac
done
if [[ "$TELEMETRY_HINT_FROM_ENV" == "0" && -f "$HOME_DIR/config.json" ]]; then
    TELEMETRY_CONFIG_VALUE="$(sed -nE 's/.*"telemetry_enabled"[[:space:]]*:[[:space:]]*(true|false).*/\1/p' "$HOME_DIR/config.json" | tail -n 1)"
    case "$TELEMETRY_CONFIG_VALUE" in
        true) TELEMETRY_HINT_ENABLED=1 ;;
        false) TELEMETRY_HINT_ENABLED=0 ;;
    esac
fi
if [[ "$TELEMETRY_HINT_ENABLED" == "1" ]]; then
    mkdir -p "$HOME_DIR"
    printf '%s\n' "$INSTALL_CHANNEL" > "$HOME_DIR/.telemetry_install_channel"
else
    rm -f "$HOME_DIR/.telemetry_install_channel"
fi

# macOS: install the .app to /Applications first, then symlink the
# bin into the bundle so `~/.local/bin/cua-driver` resolves into
# `/Applications/CuaDriver.app/Contents/MacOS/cua-driver`. The
# `realpath` walk in `is_executable_inside_cuadriver_app()` keys on
# that resolved path to know whether the auto-relaunch heuristic
# should fire. Same path and same bundle id as the Swift `cua-driver`
# install (`/Applications/CuaDriver.app`, `com.trycua.driver`). The installer
# also proves whether the replacement satisfies the previous app's designated
# requirement; the bundle identifier alone is not enough to preserve TCC.
#
# The macOS path intentionally does NOT use the
# $HOME_DIR/packages/releases/<v>/ + current symlink layout used on
# Linux. Reason: /Applications/CuaDriver.app placement is the
# anchor for both TCC attribution (cdhash + bundle id) and
# LaunchServices' `open -a CuaDriver` discovery — symlinking the
# .app from /Applications to a versioned dir under $HOME_DIR breaks
# both. The asymmetry is deliberate; rollback on macOS = reinstall
# an older release tag.
#
# Linux: drop the binary into the per-version dir under
# $HOME_DIR/packages/releases/<version>-<target>/ and swap the
# `current` symlink atomically. The visible $BIN_DIR/cua-driver
# symlinks into `current` so PATH consumers (and MCP client configs)
# never need to change when the active version moves.
#
# Fail fast on Darwin if the .app is missing — falling through to the
# bare-binary install would silently produce a CLI that can never
# auto-relaunch into a TCC-correct daemon. CodeRabbit #3.
if [[ "$OS" == "Darwin" ]]; then
    if [[ -z "${SRC_APP:-}" || ! -d "$SRC_APP" ]]; then
        err "macOS install requires the .app bundle (SRC_APP not found at ${SRC_APP:-<unset>})"
        err "  This usually means the downloaded tarball is missing CuaDriver.app — re-run the installer or"
        err "  pin a known-good release via CUA_DRIVER_RS_VERSION=<version>."
        exit 1
    fi
fi
DAEMONS_STOPPED_BEFORE_SWAP=0
if [[ "$OS" == "Darwin" && -n "$SRC_APP" && -d "$SRC_APP" ]]; then
    if [[ ! -w "/Applications" ]]; then
        err "/Applications is not writable. Re-run this installer in a shell where it is, or grant write access."
        err "  Without the .app bundle, \`cua-driver-rs mcp\` from an IDE terminal will not auto-relaunch into a TCC-correct daemon."
        exit 1
    fi
    if ! command -v codesign >/dev/null 2>&1; then
        err "codesign is required to verify the macOS release app safely"
        exit 1
    fi
    if ! codesign --verify --deep --strict "$SRC_APP" 2>/dev/null; then
        err "downloaded CuaDriver.app failed signature verification; the installed app was not changed"
        exit 1
    fi
    STAGED_BUNDLE_ID=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' \
        "$SRC_APP/Contents/Info.plist" 2>/dev/null || true)
    if [[ "$STAGED_BUNDLE_ID" != "com.trycua.driver" ]]; then
        err "downloaded app has unexpected bundle id ${STAGED_BUNDLE_ID:-<missing>}; the installed app was not changed"
        exit 1
    fi
    STAGED_REQUIREMENT="$(macos_designated_requirement "$SRC_APP" || true)"
    if [[ -z "$STAGED_REQUIREMENT" ]]; then
        err "could not read the downloaded app's designated requirement; the installed app was not changed"
        exit 1
    fi

    REPLACED_SWIFT=0
    PREVIOUS_REQUIREMENT=""
    REQUIREMENT_COMPATIBILITY="unknown"
    if [[ -e "$APP_DEST" ]]; then
        PREV_BUNDLE_ID=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$APP_DEST/Contents/Info.plist" 2>/dev/null || true)
        PREV_BUNDLE_VERSION=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP_DEST/Contents/Info.plist" 2>/dev/null || true)
        if [[ "$PREV_BUNDLE_ID" == "com.trycua.driver" ]] && [[ -n "$PREV_BUNDLE_VERSION" ]]; then
            log "replacing existing cua-driver at $APP_DEST (${PREV_BUNDLE_ID}, version ${PREV_BUNDLE_VERSION})"
            REPLACED_SWIFT=1
        elif [[ -n "$PREV_BUNDLE_ID" ]]; then
            log "replacing existing $APP_DEST (bundle id $PREV_BUNDLE_ID)"
        else
            log "removing existing $APP_DEST"
        fi
        if [[ "$PREV_BUNDLE_ID" == "com.trycua.driver" ]] \
           && codesign --verify --deep --strict "$APP_DEST" 2>/dev/null; then
            PREVIOUS_REQUIREMENT="$(macos_designated_requirement "$APP_DEST" || true)"
            if [[ -n "$PREVIOUS_REQUIREMENT" ]]; then
                REQUIREMENT_COMPATIBILITY="$(macos_requirement_compatibility \
                    "$PREVIOUS_REQUIREMENT" "$SRC_APP")"
            else
                log "warning: could not read the existing app's designated requirement; preserving TCC rows because compatibility is unknown"
            fi
        elif [[ "$PREV_BUNDLE_ID" == "com.trycua.driver" ]]; then
            log "warning: existing CuaDriver.app signature could not be verified; preserving TCC rows because compatibility is unknown"
        else
            log "warning: the existing app does not own com.trycua.driver; it will not be used to decide whether Cua Driver TCC rows are stale"
        fi
    fi

    # Stop the old daemon while its verified bundle still exists. This avoids
    # leaving a running process whose executable path disappears mid-upgrade.
    stop_cua_driver_daemons
    show_cua_driver_daemon_survivors
    DAEMONS_STOPPED_BEFORE_SWAP=1

    MACOS_APP_BACKUP="${APP_DEST}.install-backup.$$"
    if [[ -e "$MACOS_APP_BACKUP" ]]; then
        err "temporary backup path already exists: $MACOS_APP_BACKUP"
        exit 1
    fi
    if [[ -e "$APP_DEST" ]]; then
        MACOS_APP_HAD_PREVIOUS=1
    fi
    MACOS_APP_SWAP_STARTED=1
    if [[ "$MACOS_APP_HAD_PREVIOUS" == "1" ]]; then
        mv "$APP_DEST" "$MACOS_APP_BACKUP"
    fi
    log "installing $APP_DEST"
    # `ditto` preserves the bundle's metadata + nested symlinks the way
    # Apple's installer would. `cp -R` works but doesn't preserve as
    # much, and ditto is always present on macOS.
    INSTALL_VALID=0
    if ditto "$SRC_APP" "$APP_DEST" \
       && codesign --verify --deep --strict "$APP_DEST" 2>/dev/null; then
        INSTALLED_BUNDLE_ID=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' \
            "$APP_DEST/Contents/Info.plist" 2>/dev/null || true)
        INSTALLED_REQUIREMENT="$(macos_designated_requirement "$APP_DEST" || true)"
        if [[ "$INSTALLED_BUNDLE_ID" == "$STAGED_BUNDLE_ID" \
           && -n "$INSTALLED_REQUIREMENT" \
           && "$INSTALLED_REQUIREMENT" == "$STAGED_REQUIREMENT" ]]; then
            INSTALL_VALID=1
        fi
    fi
    if [[ "$INSTALL_VALID" != "1" ]]; then
        rm -rf "$APP_DEST"
        if [[ -e "$MACOS_APP_BACKUP" ]]; then
            mv "$MACOS_APP_BACKUP" "$APP_DEST"
        fi
        err "installed CuaDriver.app did not preserve its verified signing identity; the replacement was rolled back"
        exit 1
    fi
    APP_BINARY="$APP_DEST/Contents/MacOS/$BINARY_NAME"
    if [[ ! -x "$APP_BINARY" ]]; then
        rm -rf "$APP_DEST"
        if [[ -e "$MACOS_APP_BACKUP" ]]; then
            mv "$MACOS_APP_BACKUP" "$APP_DEST"
        fi
        err "binary missing at $APP_BINARY; the replacement was rolled back"
        exit 1
    fi

    # Register synchronously so both `open -a CuaDriver` and `tccutil reset`
    # resolve the replacement bundle rather than a stale LaunchServices entry.
    LSREGISTER="/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister"
    if [[ -x "$LSREGISTER" ]] \
       && "$LSREGISTER" -f "$APP_DEST" >/dev/null 2>&1; then
        :
    else
        rm -rf "$APP_DEST"
        if [[ -e "$MACOS_APP_BACKUP" ]]; then
            mv "$MACOS_APP_BACKUP" "$APP_DEST"
            "$LSREGISTER" -f "$APP_DEST" >/dev/null 2>&1 || true
        fi
        err "could not register the replacement app with LaunchServices; the replacement was rolled back"
        exit 1
    fi

    # Re-check the installed copy against the old requirement. A disagreement
    # with the staged result means the copy did not preserve the expected code
    # identity and must not trigger a destructive permission reset.
    if [[ -n "$PREVIOUS_REQUIREMENT" ]]; then
        INSTALLED_COMPATIBILITY="$(macos_requirement_compatibility \
            "$PREVIOUS_REQUIREMENT" "$APP_DEST")"
        if [[ "$INSTALLED_COMPATIBILITY" != "$REQUIREMENT_COMPATIBILITY" ]]; then
            rm -rf "$APP_DEST"
            if [[ -e "$MACOS_APP_BACKUP" ]]; then
                mv "$MACOS_APP_BACKUP" "$APP_DEST"
            fi
            if [[ "$INSTALLED_COMPATIBILITY" == "unknown" ]]; then
                err "could not re-verify the installed app's signing compatibility; the replacement was rolled back"
            else
                err "installed app's signing compatibility changed during copy; the replacement was rolled back"
            fi
            exit 1
        fi
    fi

    MACOS_APP_INSTALL_COMMITTED=1
    rm -rf "$MACOS_APP_BACKUP" || true
    ln -sf "$APP_BINARY" "$BIN_LINK"
    log "symlinked $BIN_LINK -> $APP_BINARY"
    if ! macos_reset_tcc_after_requirement_change "$REQUIREMENT_COMPATIBILITY"; then
        exit 1
    fi
else
    # Linux: versioned-dirs + atomic `current` symlink swap.
    #
    # Layout under $HOME_DIR/packages/:
    #   releases/<version>-<target>/cua-driver   (this install)
    #   releases/<older>-<target>/cua-driver     (kept for rollback)
    #   current/cua-driver -> ../releases/<active>-<target>/cua-driver
    #
    # Swap mechanics: write the new symlink to `current.tmp`, then
    # `mv -Tf current.tmp current` so the rename is a single
    # filesystem call. A daemon that already mmap'd the previous
    # `current/cua-driver` keeps using the open file handle — Unix
    # only invalidates path-based lookups, not held fds.
    PACKAGES_DIR="$HOME_DIR/packages"
    RELEASES_DIR="$PACKAGES_DIR/releases"
    CURRENT_LINK="$PACKAGES_DIR/current"
    VERSIONED_DIR="$RELEASES_DIR/${VERSION}-${TARGET}"

    mkdir -p "$VERSIONED_DIR"
    install -m 0755 "$SRC" "$VERSIONED_DIR/$BINARY_NAME"
    if [[ "$THEME_AVAILABLE" == "1" ]]; then
        install -m 0755 "$SRC_THEME" "$VERSIONED_DIR/cua-cursor-theme"
    fi
    if [[ -d "${SRC_WAYLAND_HELPER:-}" ]]; then
        mkdir -p "$VERSIONED_DIR/wayland-helper"
        cp -R "$SRC_WAYLAND_HELPER/." "$VERSIONED_DIR/wayland-helper/"

        INSTALLED_WAYLAND_HELPER="${XDG_DATA_HOME:-$HOME/.local/share}/gnome-shell/extensions/winrects@cua"
        if [[ -d "$INSTALLED_WAYLAND_HELPER" ]]; then
            cp "$SRC_WAYLAND_HELPER/winrects@cua/metadata.json" \
                "$SRC_WAYLAND_HELPER/winrects@cua/extension.js" \
                "$INSTALLED_WAYLAND_HELPER/"
            log "updated installed GNOME helper; reload the GNOME session to activate it"
        fi
    fi
    log "installed $VERSIONED_DIR/$BINARY_NAME (version $VERSION, target $TARGET)"

    # `ln -sfn` would replace an existing dir-symlink in place but is
    # not atomic on Linux (it unlinks then symlinks). Use a tmp symlink
    # + atomic rename instead so a concurrent `cua-driver` lookup
    # always sees either the old or new target, never an absent path.
    TMP_LINK="$PACKAGES_DIR/.current.$$"
    rm -rf "$TMP_LINK"
    # Relative target so the link is portable if $HOME_DIR is moved.
    ln -s "releases/${VERSION}-${TARGET}" "$TMP_LINK"
    # `mv -Tf` is the atomic-rename form on GNU coreutils (Linux). On
    # BSD mv (macOS — which doesn't take this branch in production, but
    # we still want this script to be runnable from a macOS dev shell
    # for testing) `-T` is unknown; fall back to a non-atomic rm+mv.
    if ! mv -Tf "$TMP_LINK" "$CURRENT_LINK" 2>/dev/null; then
        rm -rf "$CURRENT_LINK"
        mv "$TMP_LINK" "$CURRENT_LINK"
    fi
    log "current -> releases/${VERSION}-${TARGET}"

    # Visible PATH entry: replace whatever was at $BIN_LINK (could be
    # an old plain binary from a pre-versioned-dirs install) with a
    # symlink into `current`.
    rm -f "$BIN_LINK"
    ln -s "$CURRENT_LINK/$BINARY_NAME" "$BIN_LINK"
    log "symlinked $BIN_LINK -> $CURRENT_LINK/$BINARY_NAME"

    # Post-install GC of old per-version release dirs. Runs AFTER the
    # atomic `current` swap above so the about-to-be-active version is
    # never a deletion candidate (it's both the newest by mtime and
    # exempted via the current-symlink check inside prune_old_releases).
    prune_old_releases "$RELEASES_DIR" "$CURRENT_LINK" "$TARGET" "$KEEP_VERSIONS"
fi

# --- Sweep the legacy ~/.cua-driver-rs home -----------------------------
#
# This release installer used to default HOME_DIR to ~/.cua-driver-rs (the
# pre-v0.2.16 name). Now that it writes to ~/.cua-driver like install-local
# and the runtime, a prior RELEASE install can have left a stale
# ~/.cua-driver-rs behind — the source of the two-homes collision this PR
# fixes. Sweep it now that the new install is fully staged under the canonical
# home, mirroring the same belt-and-braces sweep _install-local-rust.sh does.
# Runs AFTER staging so we never delete state before the replacement exists;
# skipped when the user pinned CUA_DRIVER_RS_HOME to the legacy path on
# purpose. Best-effort + idempotent.
if [[ -d "$LEGACY_HOME_DIR" && "$HOME_DIR" != "$LEGACY_HOME_DIR" ]]; then
    mkdir -p "$HOME_DIR"
    for telemetry_file in .telemetry_id .installation_recorded; do
        if [[ -f "$LEGACY_HOME_DIR/$telemetry_file" && ! -e "$HOME_DIR/$telemetry_file" ]]; then
            cp -p "$LEGACY_HOME_DIR/$telemetry_file" "$HOME_DIR/$telemetry_file" 2>/dev/null \
                && log "preserved legacy telemetry state $telemetry_file" \
                || log "note: could not preserve legacy telemetry state $telemetry_file"
        fi
    done
    rm -rf "$LEGACY_HOME_DIR" 2>/dev/null \
        && log "swept legacy package home $LEGACY_HOME_DIR (reconciled onto $HOME_DIR)" \
        || log "note: could not fully remove legacy package home $LEGACY_HOME_DIR (best-effort)"
fi

# --- Stop any pre-swap cua-driver daemons -------------------------------
#
# Mirror of install.ps1's `Stop-CuaDriverDaemons` call sequence. The
# new binary is now in place under packages/current/ (Linux) or
# /Applications/CuaDriver.app (macOS) — kill any in-memory daemon
# that was holding the OLD binary so the next `cua-driver` invocation
# picks up the freshly-installed code. Without this, an autostart
# LaunchAgent / systemd user unit / manual `serve` shell keeps serving
# pre-upgrade behaviour until logout, which is what surfaces to users
# as "the bug I just fixed is still there".
if [[ "$DAEMONS_STOPPED_BEFORE_SWAP" != "1" ]]; then
    stop_cua_driver_daemons
    show_cua_driver_daemon_survivors
fi

# Agent skill pack: NOT auto-linked. The install script never touches
# ~/.claude/skills/, ~/.agents/skills/, etc. Run `cua-driver skills
# install` after install to fetch + symlink the skill pack from the
# matching GitHub release. The post-install hint below points at the
# verb.

# --- Record consent-aware install telemetry -----------------------------
#
# Telemetry is default-on, but the binary applies the same effective consent
# policy to installation events as every other event: environment override,
# then the persisted preference, then the default. It records the pseudonymous
# installation once and the installed release once per version. The channel
# is a fixed enum so an inherited/user-controlled value cannot fragment the
# dashboard.
#
# Background + redirect so a slow / failed POST never blocks the install.
echo "Telemetry defaults to enabled for new installations; saved preferences and environment overrides are honored."
echo "When enabled, Cua collects a pseudonymous installation ID and bounded, content-free usage metadata."
echo "  No prompts, tool arguments, screen contents, or file paths are collected."
echo "  Disable persistently at any time: $BIN_LINK telemetry disable"

CUA_DRIVER_INSTALL_CHANNEL="$INSTALL_CHANNEL" \
CUA_DRIVER_RELEASE_VERSION="$VERSION" \
    "$BIN_LINK" telemetry install-event >/dev/null 2>&1 &
disown 2>/dev/null || true

# Auto-extend PATH for users whose shell doesn't already include BIN_DIR.
if [[ "$NO_MODIFY_PATH" != "1" ]] && [[ ":$PATH:" != *":$BIN_DIR:"* ]]; then
    SHELL_RC=""
    case "${SHELL:-}" in
        */zsh)  SHELL_RC="$HOME/.zshrc"  ;;
        */bash) SHELL_RC="$HOME/.bashrc" ;;
    esac
    if [[ -n "$SHELL_RC" ]]; then
        {
            printf '\n# Added by cua-driver-rs installer — see https://github.com/trycua/cua\n'
            printf 'export PATH="%s:$PATH"\n' "$BIN_DIR"
        } >> "$SHELL_RC"
        log "appended PATH update to $SHELL_RC — open a new shell or run \`source $SHELL_RC\`"
    else
        log "WARNING: $BIN_DIR is not on PATH; add it manually."
    fi
fi

echo ""
echo "cua-driver-rs $VERSION installed."
echo ""

if [[ "${REPLACED_SWIFT:-0}" == "1" ]]; then
    echo "Upgraded the cua-driver bundle that was previously at $APP_DEST."
    case "${REQUIREMENT_COMPATIBILITY:-unknown}" in
        compatible)
            echo "Verified that the replacement satisfies the previous code-signing"
            echo "requirement, so existing Accessibility and Screen Recording grants"
            echo "were preserved."
            ;;
        incompatible)
            echo "The code-signing requirement changed, so stale Accessibility and"
            echo "Screen Recording rows were cleared. Re-authorize the new app with:"
            echo "  cua-driver permissions grant"
            ;;
        *)
            echo "The previous code-signing requirement could not be verified. Existing"
            echo "TCC rows were left unchanged to avoid destroying valid grants."
            ;;
    esac
    echo ""
fi

# Unified post-install hints come from a single shared text file so the
# 4 Rust installers (this script + install.ps1 + install-local.sh +
# install-local.ps1) never drift. The .txt holds the OS-agnostic bulk
# (Try-it / skill pack / MCP setup / docs link) with {{BINARY}}
# placeholders; OS-specific bits (autostart / TCC) stay inline below
# in each installer where they're per-shell natural.
HINTS_URL="https://cua.ai/driver/post-install-hints.txt"
HINTS_TXT="$TMP_DIR/post-install-hints.txt"
if curl -fsSL "$HINTS_URL" -o "$HINTS_TXT" 2>/dev/null && [ -s "$HINTS_TXT" ]; then
    sed "s|{{BINARY}}|$BIN_LINK|g" "$HINTS_TXT"
else
    # Network fetch failed — print a one-line essentials fallback so the
    # user always gets enough to recover, even if hint-text fetching is
    # blocked. Skip everything else.
    echo "Next steps: $BIN_LINK --version  |  $BIN_LINK mcp-config  |  $BIN_LINK skills install"
    echo "Docs: https://github.com/trycua/cua/tree/main/libs/cua-driver/rust"
fi

case "$(uname -s)" in
    Darwin)
        echo ""
        echo "macOS TCC: grant Accessibility + Screen Recording on first run:"
        echo "  open -n -g -a CuaDriver --args serve"
        echo "  $BIN_LINK check_permissions"
        ;;
    Linux)
        echo ""
        echo "Auto-start at logon (optional):"
        echo "  Re-run the local installer with --autostart to register a systemd user unit."
        ;;
esac
