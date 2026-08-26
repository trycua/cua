#!/usr/bin/env bash
# cua-driver uninstaller (Rust implementation only). Mirrors uninstall.ps1 on
# Windows: one canonical script per shell, no private `_uninstall-rust.sh`
# helper.
#
# Behaviour by host + flag:
#   all hosts + no flag             → Rust uninstall
#   --backend=rust/swift            → no-op (Rust is the only supported backend)
#   --experimental-rust             → legacy alias (no-op)
#
# Swift uninstall removes:
#   - ~/.local/bin/cua-driver symlink (+ legacy /usr/local/bin/cua-driver)
#   - /Applications/CuaDriver.app bundle
#   - ~/.cua-driver/ (telemetry id + install marker)
#   - ~/Library/Application Support/Cua Driver/ (config.json)
#   - ~/Library/Caches/cua-driver/ (daemon/cache state)
#   - Skill symlinks under ~/.claude/skills/cua-driver, ~/.agents/skills/
#     cua-driver, ~/.openclaw/skills/cua-driver, ~/.config/opencode/
#     skills/cua-driver (only when they point at our app bundle)
#   - Claude MCP registrations in ~/.claude.json (cua-driver / cua-computer-use)
#
# Rust uninstall removes:
#   All hosts:
#     - the release supervisor and daemon are proven stopped before any files
#       are removed. A live PID must be owned by the installed release before
#       the trusted installed `cua-driver stop` path may escalate that PID.
#   Linux:
#     - ~/.local/bin/cua-driver symlink (only when it resolves to a
#       cua-driver path — a Swift-driver symlink is left in place)
#     - versioned packages/current symlink under ~/.cua-driver/
#     - telemetry id, preference, and registration markers are preserved by
#       default so a reinstall remains the same pseudonymous installation
#     - ~/.config/systemd/user/cua-driver.service (if --autostart
#       was used via install-local.sh — stop + disable + remove), plus the
#       legacy cua-driver-rs.service unit
#     - Skill symlinks under ~/.claude/skills/cua-driver(-rs), ~/.agents/
#       skills/…, ~/.openclaw/skills/…, ~/.config/opencode/skills/…
#   macOS:
#     - /Applications/CuaDriver.app bundle (+ legacy CuaDriverRs.app)
#     - ~/.local/bin/cua-driver symlink (only when it resolves into
#       /Applications/CuaDriver.app)
#     - runtime payloads under ~/.cua-driver/ and legacy ~/.cua-driver-rs/;
#       telemetry state remains unless --purge is passed
#     - ~/Library/LaunchAgents/com.trycua.cua-driver.plist (if --autostart
#       was used via install-local.sh — unload + remove), plus the legacy
#       com.trycua.cua-driver-rs.plist LaunchAgent
#     - Skill symlinks under ~/.claude/skills/cua-driver(-rs), etc.
#
# Shared-path safety: /Applications/CuaDriver.app + its ~/.local/bin
# symlink use the same bundle id (com.trycua.driver) as the Swift driver,
# so they're only removed when an unambiguous Rust marker is on disk
# (~/.cua-driver/packages/, legacy ~/.cua-driver-rs/, CuaDriverRs.app,
# the LaunchAgent/systemd unit, or current Rust telemetry state).
#
# Also scrubs Claude MCP registrations in ~/.claude.json that match
# the active backend.
#
# Revokes TCC grants on macOS by default (Accessibility + Screen Recording)
# so the next install prompts cleanly under the new signing identity. Pass
# --keep-tcc to preserve grants across uninstall/reinstall.
#
# Usage:
#   /bin/bash -c "$(curl -fsSL https://cua.ai/driver/uninstall.sh)"
#   /bin/bash -c "$(curl -fsSL https://cua.ai/driver/uninstall.sh)" -- --purge
#
# Env overrides (mirror install side):
#   CUA_DRIVER_HOME       Rust package home to remove (default ~/.cua-driver)
#   CUA_DRIVER_RS_HOME    Legacy alias for CUA_DRIVER_HOME
set -euo pipefail

# ----------------------------------------------------------------------
# Flag parsing — same two-pass shape as install.sh so the argv shapes
# stay bit-compatible across install/uninstall and a future Rust-only
# flag flows through without edits.
# ----------------------------------------------------------------------
USE_RUST_BACKEND=1
RESET_TCC=1
PURGE_DATA=0
FORWARDED_ARGS=()
PASSTHROUGH=0
while [[ $# -gt 0 ]]; do
    if [[ "$PASSTHROUGH" == "1" ]]; then
        FORWARDED_ARGS+=("$1"); shift; continue
    fi
    case "$1" in
        --experimental-rust) shift ;;  # legacy alias for default Rust path
        --backend=rust)      shift ;;
        --backend=swift)     shift ;;  # retired Swift (no-op)
        --reset-tcc)         RESET_TCC=1; shift ;;  # legacy/explicit default: revoke TCC grants
        --keep-tcc)          RESET_TCC=0; shift ;;  # preserve TCC grants across reinstall
        --purge)             PURGE_DATA=1; shift ;;  # also delete pseudonymous identity + preference
        --backend=*)
            printf 'error: unknown backend %q; supported: rust\n' "${1#*=}" >&2
            exit 2
            ;;
        --)                  PASSTHROUGH=1; shift ;;  # forward the rest verbatim
        *)                   FORWARDED_ARGS+=("$1"); shift ;;
    esac
done

# Legacy --backend=swift is accepted as a no-op for backward compat.
OS="$(uname -s 2>/dev/null || echo unknown)"
if [[ "$USE_RUST_BACKEND" == "1" ]]; then
    if [[ "$OS" != "Darwin" ]]; then
        printf 'note: detected non-macOS host (%s); uninstalling cua-driver via the Rust implementation.\n' "$OS" >&2
    else
        printf 'note: uninstalling cua-driver via the Rust implementation.\n' >&2
    fi
fi

# ----------------------------------------------------------------------
# Shared helpers
# ----------------------------------------------------------------------
log() { printf '==> %s\n' "$*"; }

purge_macos_history() {
    local app_bundle="$1"
    local helper="$2"
    local rust_install_present="$3"
    local codesign_tool="$4"
    if [[ "$rust_install_present" != "1" || ! -x "$helper" ]] \
        || ! "$codesign_tool" --verify --deep --strict "$app_bundle" >/dev/null 2>&1; then
        printf 'history_purge_incomplete: installed signed Cua Driver helper unavailable; preserved history state for retry\n' >&2
        return 1
    fi
    "$helper" stop >/dev/null 2>&1 || true
    if ! "$helper" history purge-offline --yes; then
        printf 'history_purge_incomplete: exact-namespace key destruction was not verified; preserved history state and app for retry\n' >&2
        return 1
    fi
}

purge_linux_history() {
    local helper="$1"
    local rust_install_present="$2"
    if [[ "$rust_install_present" != "1" || ! -x "$helper" ]]; then
        printf 'history_purge_incomplete: installed Cua Driver helper unavailable; preserved history state for retry\n' >&2
        return 1
    fi
    "$helper" stop >/dev/null 2>&1 || true
    if ! "$helper" history purge-offline --yes; then
        printf 'history_purge_incomplete: exact-namespace Secret Service key destruction was not verified; preserved history state and runtime for retry\n' >&2
        return 1
    fi
}

daemon_pid_file_path() {
    case "$OS" in
        Darwin) printf '%s/Library/Caches/cua-driver/cua-driver.pid' "$HOME" ;;
        Linux)  printf '%s/.cache/cua-driver/cua-driver.pid' "$HOME" ;;
        *)      printf '/tmp/cua-driver.pid' ;;
    esac
}

read_daemon_pid() {
    local pid
    [[ -r "$1" ]] || return 1
    IFS= read -r pid < "$1" || true
    [[ "$pid" =~ ^[1-9][0-9]*$ ]] || return 1
    printf '%s' "$pid"
}

daemon_pid_alive() { kill -0 "$1" 2>/dev/null; }

daemon_wait_for_exit() {
    local pid="$1" attempts=0
    while daemon_pid_alive "$pid"; do
        [[ "$attempts" -lt 20 ]] || return 1
        sleep 0.1 2>/dev/null || sleep 1 || true
        attempts=$((attempts + 1))
    done
}

daemon_process_identity() {
    local pid="$1" identity=""
    if [[ -L "/proc/$pid/exe" ]]; then
        identity="$(readlink "/proc/$pid/exe" 2>/dev/null || true)"
        identity="${identity% (deleted)}"
    fi
    if [[ -z "$identity" ]] && command -v ps >/dev/null 2>&1; then
        identity="$(ps -ww -o command= -p "$pid" 2>/dev/null || true)"
    fi
    identity="${identity#"${identity%%[![:space:]]*}"}"
    identity="${identity%"${identity##*[![:space:]]}"}"
    [[ -n "$identity" ]] || return 1
    printf '%s' "$identity"
}

daemon_identity_matches_install() {
    local identity="$1" executable
    for executable in "${DAEMON_EXECUTABLES[@]:-}"; do
        [[ -n "$executable" ]] || continue
        case "$identity" in
            "$executable"|"$executable"[[:space:]]*) return 0 ;;
        esac
    done
    case "$identity" in
        "$HOME_DIR"/packages/releases/*/cua-driver|"$HOME_DIR"/packages/releases/*/cua-driver[[:space:]]*) return 0 ;;
        "$LEGACY_HOME_DIR"/packages/releases/*/cua-driver|"$LEGACY_HOME_DIR"/packages/releases/*/cua-driver[[:space:]]*) return 0 ;;
    esac
    return 1
}

daemon_pid_is_release() {
    local identity
    identity="$(daemon_process_identity "$1" 2>/dev/null || true)"
    [[ -n "$identity" ]] && daemon_identity_matches_install "$identity"
}

# Missing/stale PID files never trigger a signal. This check only decides
# whether uninstall can safely continue or must preserve the runtime and abort.
release_daemon_fallback_pids() {
    command -v pgrep >/dev/null 2>&1 || return 2
    command -v ps >/dev/null 2>&1 || return 2
    local uid pid identity
    uid="$(id -u 2>/dev/null || true)"
    [[ "$uid" =~ ^[0-9]+$ ]] || return 2
    while read -r pid; do
        [[ "$pid" =~ ^[1-9][0-9]*$ ]] || continue
        identity="$(daemon_process_identity "$pid" 2>/dev/null || true)"
        [[ -n "$identity" ]] && daemon_identity_matches_install "$identity" && printf '%s\n' "$pid"
    done < <(pgrep -U "$uid" -f '(^|[[:space:]/])cua-driver([[:space:]]|$)' 2>/dev/null || true)
}

verify_release_daemon_absent() {
    local pids="" status=0
    pids="$(release_daemon_fallback_pids 2>/dev/null)" || status=$?
    if [[ "$status" == "2" ]]; then
        printf 'daemon_stop_incomplete: pgrep and ps are required to verify release daemon shutdown.\n' >&2
        return 1
    fi
    if [[ -n "$pids" ]]; then
        printf 'daemon_stop_incomplete: release cua-driver process remains but cannot be safely classified or signalled.\n' >&2
        return 1
    fi
}

stop_release_daemon() {
    DAEMON_STOP_RESULT=none
    local pid="" helper="${DAEMON_STOP_HELPER:-}" pid_file="${DAEMON_PID_FILE:-}"

    if [[ -n "$pid_file" ]] && pid="$(read_daemon_pid "$pid_file" 2>/dev/null)" \
        && daemon_pid_alive "$pid" && daemon_pid_is_release "$pid"; then
        if [[ -z "$helper" || ! -x "$helper" ]]; then
            printf 'daemon_stop_incomplete: trusted installed cua-driver stop helper is unavailable for pid %s.\n' "$pid" >&2
            DAEMON_STOP_RESULT=failed
            return 1
        fi
        "$helper" stop >/dev/null 2>&1 || true
        if ! daemon_wait_for_exit "$pid"; then
            kill -TERM "$pid" 2>/dev/null || true
            if ! daemon_wait_for_exit "$pid"; then
                kill -KILL "$pid" 2>/dev/null || true
                if ! daemon_wait_for_exit "$pid"; then
                    printf 'daemon_stop_incomplete: validated release daemon pid %s is still running.\n' "$pid" >&2
                    DAEMON_STOP_RESULT=failed
                    return 1
                fi
            fi
        fi
        DAEMON_STOP_RESULT=stopped
    fi

    # A stale/missing/foreign PID file reaches only this inspection path. The
    # same check after a valid stop catches supervisor respawn before deletion.
    sleep 0.2 2>/dev/null || true
    if ! verify_release_daemon_absent; then
        DAEMON_STOP_RESULT=failed
        return 1
    fi
    return 0
}

reject_root_invocation() {
    local effective_uid="$1"
    if [[ "$effective_uid" == "0" ]]; then
        printf 'error: do not run the Cua Driver uninstaller with sudo; run it as the login user so Computer History is purged from the correct home directory and native credential store. The script elevates only protected app removal when needed.\n' >&2
        return 77
    fi
}

# TCC revocation is on by default so uninstall leaves the next macOS install
# in a clean promptable state. The bundle id com.trycua.driver is shared with
# the retired Swift driver, so `--keep-tcc` remains available for users who
# intentionally want grants to survive uninstall/reinstall.
# When enabled, revoke Accessibility + Screen-Recording + Automation for
# com.trycua.driver. macOS-only; no-op elsewhere.
maybe_reset_tcc() {
    [[ "$RESET_TCC" == "1" ]] || return 0
    if [[ "$OS" != "Darwin" ]]; then
        log "TCC reset is macOS-only; nothing to revoke on $OS"
        return 0
    fi
    if ! command -v tccutil >/dev/null 2>&1; then
        log "TCC reset: tccutil not found; skipping"
        return 0
    fi
    # `tccutil reset <svc> com.trycua.driver` resolves the bundle id through
    # LaunchServices. If the bundle isn't registered — or this runs AFTER the
    # app was removed — tccutil fails with -10814 and the grant silently
    # survives. This MUST run while /Applications/CuaDriver.app still exists;
    # force a synchronous LaunchServices registration first so a present-but-
    # not-yet-registered bundle (fresh install race) still resolves.
    local app_bundle="/Applications/CuaDriver.app"
    if [[ -d "$app_bundle" ]]; then
        local lsregister="/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister"
        if [[ -x "$lsregister" ]]; then
            "$lsregister" -f "$app_bundle" >/dev/null 2>&1 || true
        fi
    else
        log "  warning: CuaDriver.app already removed; TCC reset may not resolve the bundle id"
    fi
    log "revoking TCC grants for com.trycua.driver"
    log "  note: com.trycua.driver is shared with the retired Swift driver;"
    log "  this clears grants for both. Pass --keep-tcc to preserve them."
    for SVC in Accessibility ScreenCapture AppleEvents; do
        if tccutil reset "$SVC" com.trycua.driver >/dev/null 2>&1; then
            log "  reset $SVC"
        else
            log "  $SVC: nothing to reset (or reset failed)"
        fi
    done
}

# Resolve a symlink target to an absolute path. realpath -e fails when
# the target is missing — we want to inspect dangling symlinks too (a
# leftover from a half-removed install should still be cleaned up), so
# fall back to readlink + manual normalize when realpath errors out.
resolve_link() {
    local link="$1"
    if [[ ! -L "$link" ]]; then
        printf ''; return 0
    fi
    local target
    if target="$(realpath "$link" 2>/dev/null)"; then
        printf '%s' "$target"
        return 0
    fi
    target="$(readlink "$link" 2>/dev/null || true)"
    case "$target" in
        /*) printf '%s' "$target" ;;
        *)  printf '%s/%s' "$(cd -- "$(dirname -- "$link")" && pwd)" "$target" ;;
    esac
}

select_daemon_stop_helper() {
    local candidate resolved
    for candidate in \
        "$APP_BUNDLE/Contents/MacOS/cua-driver" \
        "$LEGACY_APP_BUNDLE/Contents/MacOS/cua-driver" \
        "$PACKAGES_DIR/current/cua-driver" \
        "$LEGACY_HOME_DIR/packages/current/cua-driver"; do
        [[ -x "$candidate" ]] || continue
        resolved="$(realpath "$candidate" 2>/dev/null || true)"
        [[ -n "$resolved" ]] || resolved="$candidate"
        case "$resolved" in
            "$APP_BUNDLE/Contents/MacOS/cua-driver"|"$LEGACY_APP_BUNDLE/Contents/MacOS/cua-driver"|\
            "$HOME_DIR"/packages/releases/*/cua-driver|"$LEGACY_HOME_DIR"/packages/releases/*/cua-driver)
                printf '%s' "$resolved"
                return 0
                ;;
        esac
    done
    return 1
}

# Stop the native supervisor without deleting its registration. A failure here
# is fatal because deleting the runtime under a supervisor that may respawn the
# daemon is unsafe.
stop_release_supervisor() {
    local path name
    case "$OS" in
        Linux)
            for path in "$SYSTEMD_USER_UNIT" "$LEGACY_SYSTEMD_USER_UNIT"; do
                [[ -f "$path" ]] || continue
                command -v systemctl >/dev/null 2>&1 || {
                    printf 'daemon_stop_incomplete: systemd user unit exists but systemctl is unavailable.\n' >&2
                    return 1
                }
                name="${path##*/}"
                if ! systemctl --user stop "$name" >/dev/null 2>&1; then
                    printf 'daemon_stop_incomplete: failed to stop systemd user unit %s.\n' "$name" >&2
                    return 1
                fi
            done
            ;;
        Darwin)
            for path in "$LAUNCHAGENT_PLIST" "$LEGACY_LAUNCHAGENT_PLIST"; do
                [[ -f "$path" ]] || continue
                command -v launchctl >/dev/null 2>&1 || {
                    printf 'daemon_stop_incomplete: LaunchAgent exists but launchctl is unavailable.\n' >&2
                    return 1
                }
                if ! launchctl unload "$path" >/dev/null 2>&1; then
                    printf 'daemon_stop_incomplete: failed to unload LaunchAgent %s.\n' "$path" >&2
                    return 1
                fi
            done
            ;;
    esac
}

# Narrow source-only seam for the synthetic uninstall fixture. Production
# execution never sets this variable and continues through the full script.
if [[ "${CUA_DRIVER_UNINSTALL_TEST_SOURCE_ONLY:-0}" == "1" ]]; then
    return 0
fi

if ! reject_root_invocation "$(id -u)"; then
    exit 77
fi

# ----------------------------------------------------------------------
# Rust uninstall branch (default on Linux + macOS).
# ----------------------------------------------------------------------
if [[ "$USE_RUST_BACKEND" == "1" ]]; then
    USER_BIN_LINK="$HOME/.local/bin/cua-driver"
    # Canonical bundle path (post-rename — shares bundle id
    # `com.trycua.driver` with the Swift driver). The Rust install
    # replaces Swift here; both uninstallers target this path.
    APP_BUNDLE="/Applications/CuaDriver.app"
    # Legacy bundle path from earlier Rust releases that coexisted with
    # Swift under a separate name. Cleaned up if found.
    LEGACY_APP_BUNDLE="/Applications/CuaDriverRs.app"
    # Canonical package home is ~/.cua-driver (renamed from ~/.cua-driver-rs
    # in v0.2.16 / PR #1644). The old name is swept too — uninstall.sh
    # was missed in that rename and kept defaulting to the stale dir, so a
    # current install left nothing matching and the whole uninstall no-op'd.
    HOME_DIR="${CUA_DRIVER_HOME:-${CUA_DRIVER_RS_HOME:-$HOME/.cua-driver}}"
    LEGACY_HOME_DIR="$HOME/.cua-driver-rs"
    # The versioned package store (`packages/releases/*` + `current`) is
    # written only by the Rust install-local / self-updater path — it's the
    # one unambiguous on-disk Rust discriminator now that the .app bundle +
    # bundle id are shared with Swift.
    PACKAGES_DIR="$HOME_DIR/packages"
    LAUNCHAGENT_PLIST="$HOME/Library/LaunchAgents/com.trycua.cua-driver.plist"
    LEGACY_LAUNCHAGENT_PLIST="$HOME/Library/LaunchAgents/com.trycua.cua-driver-rs.plist"
    SYSTEMD_USER_UNIT="$HOME/.config/systemd/user/cua-driver.service"
    LEGACY_SYSTEMD_USER_UNIT="$HOME/.config/systemd/user/cua-driver-rs.service"
    SKILL_PACK_NAME="cua-driver"
    # Pre-rename skill pack name — swept alongside the current one so
    # users who installed under the legacy name end up clean after
    # `uninstall.sh --backend=rust`.
    LEGACY_SKILL_PACK_NAME="cua-driver-rs"

    # Rust-install marker. The Rust bundle path `/Applications/CuaDriver.app`
    # is shared with the Swift driver (same bundle id `com.trycua.driver`),
    # so we can't use that path alone as a discriminator — a Swift-only Mac
    # that runs `uninstall.sh --backend=rust` by mistake would lose its
    # Swift bundle, symlink, and Claude MCP registrations. This marker says
    # "there's at least one unambiguously-Rust artifact on disk." We gate
    # every shared-path removal below on it.
    #
    # Markers (any one suffices):
    #   - ~/.cua-driver/packages/ exists (Rust install-local / updater store)
    #   - ~/.cua-driver-rs/ exists (legacy Rust state dir, pre-rename)
    #   - /Applications/CuaDriverRs.app exists (legacy bundle, pre-rename)
    #   - current or legacy LaunchAgent plist / systemd unit exists
    #     (autostart was used)
    #   - current telemetry identity/registration marker exists (release
    #     installs on macOS live in /Applications and have no packages dir)
    RUST_INSTALL_PRESENT=0
    if [[ -d "$PACKAGES_DIR" || -d "$LEGACY_HOME_DIR" || -d "$LEGACY_APP_BUNDLE" || -f "$LAUNCHAGENT_PLIST" || -f "$LEGACY_LAUNCHAGENT_PLIST" || -f "$SYSTEMD_USER_UNIT" || -f "$LEGACY_SYSTEMD_USER_UNIT" || -f "$HOME_DIR/.telemetry_id" || -f "$HOME_DIR/.installation_recorded" ]]; then
        RUST_INSTALL_PRESENT=1
    fi

    DAEMON_PID_FILE="$(daemon_pid_file_path)"
    DAEMON_STOP_HELPER="$(select_daemon_stop_helper || true)"
    DAEMON_EXECUTABLES=(
        "$APP_BUNDLE/Contents/MacOS/cua-driver"
        "$LEGACY_APP_BUNDLE/Contents/MacOS/cua-driver"
        "$PACKAGES_DIR/current/cua-driver"
        "$LEGACY_HOME_DIR/packages/current/cua-driver"
    )
    if [[ -L "$USER_BIN_LINK" ]]; then
        DAEMON_RESOLVED="$(resolve_link "$USER_BIN_LINK")"
        case "$DAEMON_RESOLVED" in
            "$APP_BUNDLE/Contents/MacOS/cua-driver"|"$LEGACY_APP_BUNDLE/Contents/MacOS/cua-driver"|\
            "$HOME_DIR"/packages/releases/*/cua-driver|"$LEGACY_HOME_DIR"/packages/releases/*/cua-driver)
                DAEMON_EXECUTABLES+=("$USER_BIN_LINK" "$DAEMON_RESOLVED")
                ;;
        esac
        unset DAEMON_RESOLVED
    fi

    # Shutdown is the transaction boundary: supervisor + daemon must both be
    # proven stopped before any symlink, service file, bundle, or runtime file
    # is removed.
    if [[ "$RUST_INSTALL_PRESENT" == "1" ]]; then
        if ! stop_release_supervisor; then
            log "could not stop the release supervisor; runtime preserved"
            exit 1
        fi
        if ! stop_release_daemon; then
            log "could not safely stop and verify the release daemon; runtime preserved"
            exit 1
        fi
        if [[ "$DAEMON_STOP_RESULT" == "stopped" ]]; then
            log "stopped and verified the running cua-driver daemon"
        else
            log "no running release cua-driver daemon"
        fi
    else
        log "no Rust install marker; leaving any running cua-driver process untouched"
    fi

    # --- CLI symlink ---
    # Only remove ~/.local/bin/cua-driver when it resolves into a
    # cua-driver-rs install. Pre-rename installs at
    # /Applications/CuaDriverRs.app are unambiguously Rust and always
    # removed. Post-rename, the Rust install lives at
    # /Applications/CuaDriver.app — the SAME path the Swift driver
    # uses — so we only remove that link when $RUST_INSTALL_PRESENT.
    if [[ -L "$USER_BIN_LINK" ]]; then
        RESOLVED="$(resolve_link "$USER_BIN_LINK")"
        case "$RESOLVED" in
            *"CuaDriverRs.app"*|*"$HOME_DIR"*|*".cua-driver-rs"*)
                # Unambiguous Rust paths.
                rm -f "$USER_BIN_LINK"
                log "removed $USER_BIN_LINK -> $RESOLVED"
                ;;
            *"/Applications/CuaDriver.app"*)
                # Shared with the Swift driver — require a Rust marker.
                if [[ "$RUST_INSTALL_PRESENT" == "1" ]]; then
                    rm -f "$USER_BIN_LINK"
                    log "removed $USER_BIN_LINK -> $RESOLVED"
                else
                    log "$USER_BIN_LINK -> $RESOLVED (shared with Swift driver and no Rust marker on disk; skipping)"
                fi
                ;;
            *)
                log "$USER_BIN_LINK resolves to $RESOLVED (not a cua-driver-rs path; skipping)"
                ;;
        esac
    elif [[ -e "$USER_BIN_LINK" ]]; then
        log "$USER_BIN_LINK exists but is not a symlink (skipping; refusing to clobber a real file)"
    else
        log "no CLI symlink at $USER_BIN_LINK (skipping)"
    fi

    # --- Autostart (Linux systemd --user) ---
    # The stop phase already succeeded above. Disable and remove registrations
    # only after daemon shutdown has been verified.
    if [[ "$OS" == "Linux" ]]; then
        FOUND_SYSTEMD_USER_UNIT=0
        for SYSTEMD_USER_UNIT_PATH in "$SYSTEMD_USER_UNIT" "$LEGACY_SYSTEMD_USER_UNIT"; do
            if [[ -f "$SYSTEMD_USER_UNIT_PATH" ]]; then
                FOUND_SYSTEMD_USER_UNIT=1
                SYSTEMD_USER_UNIT_NAME="${SYSTEMD_USER_UNIT_PATH##*/}"
                if command -v systemctl >/dev/null 2>&1; then
                    systemctl --user disable "$SYSTEMD_USER_UNIT_NAME" 2>/dev/null || true
                fi
                rm -f "$SYSTEMD_USER_UNIT_PATH"
                log "disabled + removed systemd --user unit $SYSTEMD_USER_UNIT_NAME"
            fi
        done
        if [[ "$FOUND_SYSTEMD_USER_UNIT" == "0" ]]; then
            log "no current or legacy systemd --user unit found (skipping)"
        elif command -v systemctl >/dev/null 2>&1; then
            systemctl --user daemon-reload 2>/dev/null || true
        fi
    fi

    # --- Autostart (macOS LaunchAgent) ---
    if [[ "$OS" == "Darwin" ]]; then
        FOUND_LAUNCHAGENT_PLIST=0
        for LAUNCHAGENT_PLIST_PATH in "$LAUNCHAGENT_PLIST" "$LEGACY_LAUNCHAGENT_PLIST"; do
            if [[ -f "$LAUNCHAGENT_PLIST_PATH" ]]; then
                FOUND_LAUNCHAGENT_PLIST=1
                rm -f "$LAUNCHAGENT_PLIST_PATH"
                log "removed LaunchAgent $LAUNCHAGENT_PLIST_PATH"
            fi
        done
        if [[ "$FOUND_LAUNCHAGENT_PLIST" == "0" ]]; then
            log "no current or legacy LaunchAgent found (skipping)"
        fi
    fi

    # Cryptographic history purge must run while the exact installed helper
    # executable still exists. The helper uses the production KeyProvider and
    # its own bundle-derived namespace, then takes the exclusive writer lease;
    # failure leaves the runtime and all retryable history state in place.
    if [[ "$OS" == "Darwin" && "$PURGE_DATA" == "1" ]]; then
        HISTORY_PURGE_HELPER="$APP_BUNDLE/Contents/MacOS/cua-driver"
        if ! purge_macos_history \
            "$APP_BUNDLE" "$HISTORY_PURGE_HELPER" "$RUST_INSTALL_PRESENT" /usr/bin/codesign; then
            exit 1
        fi
        log "cryptographically purged release Computer History key and local history state"
    elif [[ "$OS" == "Darwin" ]]; then
        log "preserved encrypted Computer History if present; reinstall to reopen it or run uninstall.sh --purge to destroy it"
    elif [[ "$OS" == "Linux" && "$PURGE_DATA" == "1" ]]; then
        HISTORY_PURGE_HELPER="$PACKAGES_DIR/current/cua-driver"
        if ! purge_linux_history "$HISTORY_PURGE_HELPER" "$RUST_INSTALL_PRESENT"; then
            exit 1
        fi
        log "cryptographically purged release Computer History Secret Service key and local history state"
    elif [[ "$OS" == "Linux" ]]; then
        log "preserved encrypted Computer History if present; reinstall to reopen it or run uninstall.sh --purge to destroy it"
    fi

    # --- Revoke TCC grants BEFORE removing the app ---
    # tccutil resolves com.trycua.driver through LaunchServices, so the reset
    # only works while /Applications/CuaDriver.app is still installed. Running
    # it here (not at the closing message) is what makes the revoke actually
    # take — otherwise it fails with -10814 and the grant silently survives.
    maybe_reset_tcc

    # --- .app bundle (macOS only) ---
    # Legacy /Applications/CuaDriverRs.app is unambiguously Rust and
    # always removed when present. /Applications/CuaDriver.app is the
    # current canonical Rust path BUT also where the Swift driver
    # lives (same bundle id `com.trycua.driver`), so we only remove
    # it when $RUST_INSTALL_PRESENT — protects a Swift-only Mac from
    # losing its bundle if `uninstall.sh --experimental-rust` is run
    # by mistake.
    if [[ "$OS" == "Darwin" ]]; then
        if [[ -d "$LEGACY_APP_BUNDLE" ]]; then
            SUDO=""
            if [[ ! -w "$(dirname "$LEGACY_APP_BUNDLE")" ]]; then
                SUDO="sudo"
            fi
            $SUDO rm -rf "$LEGACY_APP_BUNDLE"
            log "removed $LEGACY_APP_BUNDLE"
        else
            log "no app bundle at $LEGACY_APP_BUNDLE (skipping)"
        fi
        if [[ -d "$APP_BUNDLE" ]]; then
            if [[ "$RUST_INSTALL_PRESENT" == "1" ]]; then
                SUDO=""
                if [[ ! -w "$(dirname "$APP_BUNDLE")" ]]; then
                    SUDO="sudo"
                fi
                $SUDO rm -rf "$APP_BUNDLE"
                log "removed $APP_BUNDLE"
            else
                log "$APP_BUNDLE exists but no Rust marker on disk (~/.cua-driver/packages/, ~/.cua-driver-rs/, CuaDriverRs.app, current/legacy LaunchAgent or systemd unit); leaving it (looks like a Swift-only install)"
            fi
        else
            log "no app bundle at $APP_BUNDLE (skipping)"
        fi
    fi

    # --- Package home ---
    # A normal uninstall deliberately keeps the pseudonymous installation ID,
    # persisted telemetry preference, and install/release markers. This lets a
    # later reinstall be counted as a returning installation without sending
    # any events while disabled. `--purge` is the explicit identity reset.
    # All removal remains gated on the Rust marker so a mistaken invocation
    # cannot damage a Swift-only Mac's shared ~/.cua-driver state.
    if [[ -d "$HOME_DIR" ]]; then
        if [[ "$RUST_INSTALL_PRESENT" == "1" || "$PURGE_DATA" == "1" ]]; then
            if [[ "$PURGE_DATA" == "1" ]]; then
                rm -rf "$HOME_DIR"
                log "purged $HOME_DIR (including telemetry identity and preference)"
            else
                # Remove only installer/runtime-owned payloads. Unknown files
                # and all telemetry state remain untouched.
                rm -rf "$HOME_DIR/packages" "$HOME_DIR/skills"
                rm -f \
                    "$HOME_DIR/.tcc-signing-identity" \
                    "$HOME_DIR/serve.out.log" \
                    "$HOME_DIR/serve.err.log"
                log "removed runtime payloads from $HOME_DIR"
                log "preserved telemetry identity, preference, and registration markers"
            fi
        else
            log "$HOME_DIR exists but no Rust marker on disk; leaving it (looks like a Swift-only / shared config dir)"
        fi
    else
        log "no package home at $HOME_DIR (skipping)"
    fi
    # Preserve legacy telemetry state during a normal uninstall so the
    # runtime's existing one-shot migration can carry the same identity into
    # ~/.cua-driver on reinstall.
    if [[ -d "$LEGACY_HOME_DIR" ]]; then
        if [[ "$PURGE_DATA" == "1" ]]; then
            rm -rf "$LEGACY_HOME_DIR"
            log "purged legacy package home $LEGACY_HOME_DIR"
        else
            rm -rf "$LEGACY_HOME_DIR/packages" "$LEGACY_HOME_DIR/skills"
            rm -f \
                "$LEGACY_HOME_DIR/.tcc-signing-identity" \
                "$LEGACY_HOME_DIR/serve.out.log" \
                "$LEGACY_HOME_DIR/serve.err.log"
            log "removed legacy runtime payloads and preserved legacy telemetry state"
        fi
    fi

    # --- Swift-era macOS data dirs (leave nothing behind) ---
    # The .app bundle + bundle id are shared with the retired Swift driver,
    # so a default (Rust) uninstall already removes the shared bundle. Sweep
    # the two Swift-only support/cache dirs here too so one `uninstall.sh`
    # leaves nothing behind regardless of which backend originally installed
    # — no second `--backend=swift` pass needed. Gated on the Rust marker
    # for the same reason the shared bundle is: a Swift-only Mac that runs
    # the default uninstall by mistake keeps its data.
    if [[ "$OS" == "Darwin" && "$RUST_INSTALL_PRESENT" == "1" ]]; then
        for SWIFT_DATA_DIR in \
            "$HOME/Library/Application Support/Cua Driver" \
            "$HOME/Library/Caches/cua-driver"; do
            if [[ -d "$SWIFT_DATA_DIR" ]]; then
                rm -rf "$SWIFT_DATA_DIR"
                log "removed $SWIFT_DATA_DIR"
            fi
        done
    fi

    # --- Agent skill symlinks ---
    # Only remove when the link is a symlink — never clobber a real
    # directory (a dev user with a hand-managed skills dir is safe).
    # We don't check the target here because `cua-driver skills install`
    # writes platform-dependent targets (the local copy under $HOME_DIR/
    # skills/cua-driver-rs/). The [[ -L ]] check is the load-bearing
    # safety bar.
    if [[ "$RUST_INSTALL_PRESENT" == "1" ]]; then
        for SKILL_LINK in \
            "$HOME/.claude/skills/$SKILL_PACK_NAME" \
            "$HOME/.agents/skills/$SKILL_PACK_NAME" \
            "$HOME/.openclaw/skills/$SKILL_PACK_NAME" \
            "$HOME/.config/opencode/skills/$SKILL_PACK_NAME" \
            "$HOME/.gemini/skills/$SKILL_PACK_NAME" \
            "$HOME/.hermes/skills/$SKILL_PACK_NAME" \
            "$HOME/.claude/skills/$LEGACY_SKILL_PACK_NAME" \
            "$HOME/.agents/skills/$LEGACY_SKILL_PACK_NAME" \
            "$HOME/.openclaw/skills/$LEGACY_SKILL_PACK_NAME" \
            "$HOME/.config/opencode/skills/$LEGACY_SKILL_PACK_NAME" \
            "$HOME/.gemini/skills/$LEGACY_SKILL_PACK_NAME" \
            "$HOME/.hermes/skills/$LEGACY_SKILL_PACK_NAME"; do
            if [[ -L "$SKILL_LINK" ]]; then
                rm -f "$SKILL_LINK"
                log "removed skill symlink $SKILL_LINK"
            elif [[ -d "$SKILL_LINK" ]]; then
                log "$SKILL_LINK is a real directory, not a symlink (skipping)"
            else
                log "no skill symlink at $SKILL_LINK (skipping)"
            fi
        done
    else
        log "no Rust install marker; leaving agent skill symlinks untouched"
    fi

    # --- Claude Code MCP registrations ---
    # Same scrub shape as the Swift branch, keyed on the cua-driver-rs
    # binary name + the per-platform install paths. Unrelated MCP
    # servers are left alone.
    CLAUDE_JSON="$HOME/.claude.json"
    if [[ -f "$CLAUDE_JSON" ]] && command -v python3 >/dev/null 2>&1; then
        PY_OUTPUT="$(
            CLAUDE_JSON="$CLAUDE_JSON" HOME_DIR="$HOME_DIR" RUST_INSTALL_PRESENT="$RUST_INSTALL_PRESENT" python3 <<'PY'
import json
import os
import shutil
import sys
import tempfile
import time

path = os.environ["CLAUDE_JSON"]
home_dir = os.environ.get("HOME_DIR", "")
rust_install_present = os.environ.get("RUST_INSTALL_PRESENT", "0") == "1"

try:
    with open(path, "r", encoding="utf-8") as f:
        data = json.load(f)
except Exception as exc:
    print(f"could not read Claude config {path}: {exc}", file=sys.stderr)
    raise SystemExit(0)

removed = []

def text_parts(value):
    if isinstance(value, str):
        return [value]
    if isinstance(value, list):
        return [item for item in value if isinstance(item, str)]
    return []

def invokes_cua_driver_rs(server):
    if not isinstance(server, dict):
        return False
    parts = []
    parts.extend(text_parts(server.get("command")))
    parts.extend(text_parts(server.get("args")))
    joined = " ".join(parts)
    # Match the Rust-port-specific anchors: bundle name, package home,
    # explicit ".cua-driver-rs" segment. Plain "cua-driver" alone is
    # ambiguous (the Swift binary uses the same filename). The shared
    # /Applications/CuaDriver.app path is ALSO ambiguous (Rust took
    # over the Swift bundle id) — only count it as Rust when a Rust
    # install marker is on disk; otherwise it is almost certainly a
    # Swift registration we should not scrub.
    if home_dir and home_dir in joined:
        return True
    if "CuaDriverRs.app" in joined or ".cua-driver-rs" in joined or "cua-driver-rs" in joined:
        return True
    if rust_install_present and "/Applications/CuaDriver.app" in joined:
        return True
    return False

def should_remove(name, server):
    return name in {"cua-driver-rs"} or invokes_cua_driver_rs(server)

def scrub_servers(servers, scope):
    if not isinstance(servers, dict):
        return
    for name in list(servers.keys()):
        if should_remove(name, servers[name]):
            del servers[name]
            removed.append(f"{scope}:{name}")

scrub_servers(data.get("mcpServers"), "user")

projects = data.get("projects")
if isinstance(projects, dict):
    for project in projects.values():
        if isinstance(project, dict):
            scrub_servers(project.get("mcpServers"), "project")

if not removed:
    raise SystemExit(0)

backup = f"{path}.bak-cua-driver-rs-uninstall-{int(time.time())}"
shutil.copy2(path, backup)

directory = os.path.dirname(path) or "."
fd, tmp_path = tempfile.mkstemp(
    prefix=".claude.json.",
    suffix=".tmp",
    dir=directory,
    text=True,
)
try:
    with os.fdopen(fd, "w", encoding="utf-8") as f:
        json.dump(data, f, indent=2, ensure_ascii=False)
        f.write("\n")
    os.replace(tmp_path, path)
except Exception:
    try:
        os.unlink(tmp_path)
    except OSError:
        pass
    raise

print(f"removed Claude MCP registration(s): {', '.join(removed)}")
print(f"backed up Claude config to {backup}")
PY
        )"
        if [[ -n "$PY_OUTPUT" ]]; then
            while IFS= read -r line; do
                log "$line"
            done <<< "$PY_OUTPUT"
        else
            log "no Claude MCP registrations for cua-driver-rs found in $CLAUDE_JSON"
        fi
    else
        log "no Claude config cleanup via python3 (missing $CLAUDE_JSON or python3)"
    fi

    # Best-effort CLI cleanup. `claude mcp remove` only touches the
    # active project / user scopes — fine to run; it's a no-op when the
    # entries were already scrubbed above.
    if command -v claude >/dev/null 2>&1; then
        for SERVER in cua-driver-rs; do
            for SCOPE in local project user; do
                if claude mcp remove "$SERVER" -s "$SCOPE" >/dev/null 2>&1; then
                    log "removed Claude MCP server $SERVER from $SCOPE scope"
                fi
            done
        done
    else
        log "claude CLI not found (skipping Claude MCP CLI cleanup)"
    fi

    # --- Closing message ---
    # Shutdown failures already abort before deletion, so reaching this point
    # means the release runtime was removed only after a verified stop.
    if [[ "$OS" == "Darwin" ]]; then
        echo ""
        echo "cua-driver uninstalled."
        if [[ "$PURGE_DATA" == "0" ]]; then
            cat << 'TELEMETRYUNMSG'

Telemetry identity and preference were preserved for a future reinstall.
To delete them too, re-run with --purge:

  /bin/bash -c "$(curl -fsSL https://cua.ai/driver/uninstall.sh)" -- --purge
TELEMETRYUNMSG
        fi
        if [[ "$RESET_TCC" != "1" ]]; then
            cat << 'FINALUNMSG'

TCC grants (Accessibility + Screen Recording) remain in System
Settings > Privacy & Security because uninstall was run with --keep-tcc.
Reset them explicitly if you want a clean re-install flow:

  tccutil reset Accessibility com.trycua.driver
  tccutil reset ScreenCapture com.trycua.driver
FINALUNMSG
        fi
    else
        echo ""
        echo "cua-driver uninstalled."
        if [[ "$PURGE_DATA" == "0" ]]; then
            cat << 'TELEMETRYUNMSG'

Telemetry identity and preference were preserved for a future reinstall.
To delete them too, re-run with --purge:

  /bin/bash -c "$(curl -fsSL https://cua.ai/driver/uninstall.sh)" -- --purge
TELEMETRYUNMSG
        fi
    fi
    exit 0
fi

USER_BIN_LINK="$HOME/.local/bin/cua-driver"
SYSTEM_BIN_LINK="/usr/local/bin/cua-driver"
APP_BUNDLE="/Applications/CuaDriver.app"
USER_DATA="$HOME/.cua-driver"
CONFIG_DIR="$HOME/Library/Application Support/Cua Driver"
CACHE_DIR="$HOME/Library/Caches/cua-driver"
# Legacy — remove if present from older installs.
LEGACY_UPDATE_SCRIPT="/usr/local/bin/cua-driver-update"
LEGACY_UPDATER_PLIST="$HOME/Library/LaunchAgents/com.trycua.cua_driver_updater.plist"

# CLI symlinks. Try the user-bin first (no sudo), then the legacy
# /usr/local/bin path (needs sudo on default macOS).
for BIN_LINK in "$USER_BIN_LINK" "$SYSTEM_BIN_LINK"; do
    if [[ -L "$BIN_LINK" ]] || [[ -e "$BIN_LINK" ]]; then
        SUDO=""
        [[ ! -w "$(dirname "$BIN_LINK")" ]] && SUDO="sudo"
        $SUDO rm -f "$BIN_LINK"
        log "removed $BIN_LINK"
    fi
done

# Legacy update script + LaunchAgent (present in installs before 0.0.6).
if [[ -f "$LEGACY_UPDATE_SCRIPT" ]]; then
    SUDO=""; [[ ! -w "$(dirname "$LEGACY_UPDATE_SCRIPT")" ]] && SUDO="sudo"
    $SUDO rm -f "$LEGACY_UPDATE_SCRIPT"
    log "removed legacy $LEGACY_UPDATE_SCRIPT"
fi
if [[ -f "$LEGACY_UPDATER_PLIST" ]]; then
    launchctl unload "$LEGACY_UPDATER_PLIST" 2>/dev/null || true
    rm -f "$LEGACY_UPDATER_PLIST"
    log "removed legacy $LEGACY_UPDATER_PLIST"
fi

# .app bundle (in /Applications, usually writable by the user).
if [[ -d "$APP_BUNDLE" ]]; then
    SUDO=""
    if [[ ! -w "$(dirname "$APP_BUNDLE")" ]]; then
        SUDO="sudo"
    fi
    $SUDO rm -rf "$APP_BUNDLE"
    log "removed $APP_BUNDLE"
else
    log "no app bundle at $APP_BUNDLE (skipping)"
fi

# User-data directory (telemetry id + install marker).
if [[ -d "$USER_DATA" ]]; then
    rm -rf "$USER_DATA"
    log "removed $USER_DATA"
else
    log "no user data at $USER_DATA (skipping)"
fi

# Persisted config.
if [[ -d "$CONFIG_DIR" ]]; then
    rm -rf "$CONFIG_DIR"
    log "removed $CONFIG_DIR"
else
    log "no config at $CONFIG_DIR (skipping)"
fi

# Cache / daemon state.
if [[ -d "$CACHE_DIR" ]]; then
    rm -rf "$CACHE_DIR"
    log "removed $CACHE_DIR"
else
    log "no cache at $CACHE_DIR (skipping)"
fi

# Agent skill symlinks (Claude Code + Codex). Only remove when the link
# is ours — a dev user pointing the symlink at a working copy of the
# repo keeps theirs untouched.
SKILL_TARGET_EXPECTED="$APP_BUNDLE/Contents/Resources/Skills/cua-driver"
for SKILL_LINK in \
    "$HOME/.claude/skills/cua-driver" \
    "$HOME/.agents/skills/cua-driver" \
    "$HOME/.openclaw/skills/cua-driver" \
    "$HOME/.config/opencode/skills/cua-driver" \
    "$HOME/.gemini/skills/cua-driver" \
    "$HOME/.hermes/skills/cua-driver"; do
    if [[ -L "$SKILL_LINK" ]] && [[ "$(readlink "$SKILL_LINK")" == "$SKILL_TARGET_EXPECTED" ]]; then
        rm -f "$SKILL_LINK"
        log "removed $SKILL_LINK"
    else
        log "no install-created skill symlink at $SKILL_LINK (skipping)"
    fi
done

# Claude Code MCP registrations. `claude mcp remove` only removes from
# the current project / user scopes, while ~/.claude.json can also
# contain stale project entries for other directories. Scrub only
# registrations explicitly named cua-driver or whose command points at
# a cua-driver binary, so unrelated servers named "computer-use" are
# left alone.
CLAUDE_JSON="$HOME/.claude.json"
if [[ -f "$CLAUDE_JSON" ]] && command -v python3 >/dev/null 2>&1; then
    PY_OUTPUT="$(
        CLAUDE_JSON="$CLAUDE_JSON" python3 <<'PY'
import json
import os
import shutil
import sys
import tempfile
import time

path = os.environ["CLAUDE_JSON"]

try:
    with open(path, "r", encoding="utf-8") as f:
        data = json.load(f)
except Exception as exc:
    print(f"could not read Claude config {path}: {exc}", file=sys.stderr)
    raise SystemExit(0)

removed = []

def text_parts(value):
    if isinstance(value, str):
        return [value]
    if isinstance(value, list):
        return [item for item in value if isinstance(item, str)]
    return []

def invokes_cua_driver(server):
    if not isinstance(server, dict):
        return False
    parts = []
    parts.extend(text_parts(server.get("command")))
    parts.extend(text_parts(server.get("args")))
    joined = " ".join(parts)
    return "cua-driver" in joined or "CuaDriver.app" in joined

def should_remove(name, server):
    return name in {"cua-driver", "cua-computer-use"} or invokes_cua_driver(server)

def scrub_servers(servers, scope):
    if not isinstance(servers, dict):
        return
    for name in list(servers.keys()):
        if should_remove(name, servers[name]):
            del servers[name]
            removed.append(f"{scope}:{name}")

scrub_servers(data.get("mcpServers"), "user")

projects = data.get("projects")
if isinstance(projects, dict):
    for project in projects.values():
        if isinstance(project, dict):
            scrub_servers(project.get("mcpServers"), "project")

if not removed:
    raise SystemExit(0)

backup = f"{path}.bak-cua-driver-uninstall-{int(time.time())}"
shutil.copy2(path, backup)

directory = os.path.dirname(path) or "."
fd, tmp_path = tempfile.mkstemp(
    prefix=".claude.json.",
    suffix=".tmp",
    dir=directory,
    text=True,
)
try:
    with os.fdopen(fd, "w", encoding="utf-8") as f:
        json.dump(data, f, indent=2, ensure_ascii=False)
        f.write("\n")
    os.replace(tmp_path, path)
except Exception:
    try:
        os.unlink(tmp_path)
    except OSError:
        pass
    raise

print(f"removed Claude MCP registration(s): {', '.join(removed)}")
print(f"backed up Claude config to {backup}")
PY
    )"
    if [[ -n "$PY_OUTPUT" ]]; then
        while IFS= read -r line; do
            log "$line"
        done <<< "$PY_OUTPUT"
    else
        log "no Claude MCP registrations for cua-driver found in $CLAUDE_JSON"
    fi
else
    log "no Claude config cleanup via python3 (missing $CLAUDE_JSON or python3)"
fi

# Best-effort CLI cleanup for the active Claude project. This covers
# .mcp.json / current-working-directory scopes when present and is
# harmless when the entries were already removed above.
if command -v claude >/dev/null 2>&1; then
    for SERVER in cua-driver cua-computer-use; do
        for SCOPE in local project user; do
            if claude mcp remove "$SERVER" -s "$SCOPE" >/dev/null 2>&1; then
                log "removed Claude MCP server $SERVER from $SCOPE scope"
            fi
        done
    done
else
    log "claude CLI not found (skipping Claude MCP CLI cleanup)"
fi

maybe_reset_tcc

echo ""
echo "cua-driver uninstalled."
if [[ "$RESET_TCC" != "1" ]]; then
    cat << 'FINALUNMSG'

TCC grants (Accessibility + Screen Recording) remain in System
Settings > Privacy & Security because uninstall was run with --keep-tcc.
Reset them explicitly if you want a clean re-install flow:

  tccutil reset Accessibility com.trycua.driver
  tccutil reset ScreenCapture com.trycua.driver
FINALUNMSG
fi