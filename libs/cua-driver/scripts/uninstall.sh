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
#     - the running `cua-driver serve` daemon is stopped before anything is
#       deleted (mirrors uninstall.ps1 on Windows). Candidates are selected by
#       argv[0]; whether one is a daemon is decided by scanning its command
#       line the way the CLI does, so `cua-driver mcp` children of a live MCP
#       client are reported, not killed, even when a flag value happens to be
#       the word `serve`. That scan reads real argv from /proc where the host
#       has it; where it does not (macOS) an unrecoverable command line is
#       reported as `daemon_stop_undetermined` instead of guessed at. A bare
#       `cua-driver` argv[0] is kept only when the executable behind the pid
#       resolves into the install. After the pids die the scan repeats, so a
#       supervisor that survived the autostart teardown is caught. A daemon
#       that outlives SIGKILL or keeps respawning is reported as
#       `daemon_stop_incomplete`, and the script then exits non-zero with a
#       closing line saying the removal completed but a process is still up.
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

# Turn a literal installed-binary path into an argv[0] regex fragment: every
# ERE metacharacter a $HOME or a package path may contain is escaped so the
# path matches itself and nothing else.
escape_path_regex() {
    printf '%s' "$1" | sed 's/[][(){}.^$*+?|\\]/\\&/g'
}

# Flags whose next token is a value rather than the subcommand. Kept in
# lockstep with VALUE_FLAGS in
# libs/cua-driver/rust/crates/cua-driver/src/cli.rs — the test suite fails when
# the two lists drift apart.
DAEMON_VALUE_FLAGS="--cursor-theme --cursor-reduced-motion --glide-ms --dwell-ms
--idle-hide-ms --screenshot-out-file --client --socket --permission-mode --grant
--session-policy --capability-manifest --pid-file --type --host-bundle-id --pid
--strategy --window-id --session --profile-mode --profile-name
--experimental-pip-geometry"

is_daemon_value_flag() {
    local candidate="$1"
    local flag
    for flag in $DAEMON_VALUE_FLAGS; do
        if [[ "$candidate" == "$flag" ]]; then
            return 0
        fi
    done
    return 1
}

# Print the subcommand a token list resolves to, or nothing when it has none.
#
# This is a scan rather than a pattern because the two are not the same
# language. The CLI takes the first NON-FLAG argument as the subcommand, and
# only a value-taking flag swallows the token after it:
#
#   cua-driver --no-overlay serve      → serve  (a daemon)
#   cua-driver --socket /tmp/x serve   → serve  (a daemon)
#   cua-driver --no-overlay call serve → call   (a finite CLI call)
#   cua-driver --socket serve mcp      → mcp    (an MCP child, socket "serve")
#
# A regex that tries to express this is ambiguous — its generic-flag branch
# also matches `--socket`, so the last two lines above match as daemons and get
# killed. Mirror `positionals()` in cli.rs instead, and let the regex do only
# what it is unambiguous at: pinning argv[0].
#
# $1 selects how much the tokens can be trusted:
#
#   exact      the tokens are the real argv, one element each.
#   flattened  the tokens came from a whitespace-joined command line, where an
#              argument that itself contains whitespace is indistinguishable
#              from the arguments after it. That is recoverable only until a
#              value-taking flag appears: `--socket "/tmp/a serve" mcp` and
#              `--socket /tmp/a serve` flatten to command lines whose token
#              lists differ by nothing a scan can see. Print `?` there —
#              guessing would either kill an MCP child or miss a daemon.
daemon_subcommand() {
    local mode="$1"
    shift
    local -a tokens=("$@")
    local index=1
    local token
    while [[ "$index" -lt "${#tokens[@]}" ]]; do
        token="${tokens[index]}"
        if is_daemon_value_flag "$token"; then
            if [[ "$mode" == "flattened" ]]; then
                printf '?'
                return 0
            fi
            index=$((index + 2))
        elif [[ "$token" == -* ]]; then
            index=$((index + 1))
        else
            printf '%s' "$token"
            return 0
        fi
    done
    return 1
}

# Print the subcommand of a running process, or `?` when its argument
# boundaries cannot be recovered.
#
# Linux exposes the real argv through /proc/<pid>/cmdline, NUL-separated, so the
# scan is exact there. macOS has no equivalent a shell can read: `ps -o args=`
# joins the arguments with spaces and the original boundaries are gone, so the
# scan falls back to `flattened` and declines to guess. A daemon started the way
# the product starts one — `serve` ahead of any flag, as install-local.sh, the
# LaunchAgent, the systemd unit and the runtime's own relaunch all do — is
# unaffected, because no value flag precedes the subcommand.
daemon_pid_subcommand() {
    local pid="$1"
    local -a argv=()
    local token
    if [[ -r "/proc/$pid/cmdline" ]]; then
        while IFS= read -r -d "" token; do
            argv+=("$token")
        done < "/proc/$pid/cmdline"
        [[ "${#argv[@]}" -gt 0 ]] || return 1
        daemon_subcommand exact "${argv[@]}"
        return
    fi
    local command_line
    command_line="$(ps -ww -o args= -p "$pid" 2>/dev/null || true)"
    [[ -n "$command_line" ]] || return 1
    read -r -a argv <<< "$command_line"
    [[ "${#argv[@]}" -gt 0 ]] || return 1
    daemon_subcommand flattened "${argv[@]}"
}

# Anchor an argv[0] regex fragment as a whole-command-line match for pgrep.
# Anchoring is what keeps a launcher shell that merely mentions the path in its
# script text out of the match — see the same note in uninstall-local.sh.
daemon_argv0_pattern() {
    printf '^%s([[:space:]]|$)' "$1"
}

# True when a pid is still running and not just an unreaped zombie. A zombie
# holds no socket, no window and no TCC connection — treating one as alive
# would turn a successful stop into a false `daemon_stop_incomplete`.
daemon_pid_alive() {
    local state
    state="$(ps -o state= -p "$1" 2>/dev/null || true)"
    state="${state//[[:space:]]/}"
    case "$state" in
        ""|Z*) return 1 ;;
        *) return 0 ;;
    esac
}

daemon_pids_alive() {
    local pid
    for pid in "$@"; do
        if daemon_pid_alive "$pid"; then
            return 0
        fi
    done
    return 1
}

# Print the path of a running process's executable, empty when the host will
# not say. Linux keeps it as /proc/<pid>/exe — still resolvable after the file
# is unlinked, which is exactly the case this phase exists for, so the
# " (deleted)" suffix is trimmed. BSD `ps -o comm=` prints the executable path
# on macOS; anything that is not absolute is treated as "unknown".
daemon_pid_executable() {
    local pid="$1"
    local executable
    if [[ -L "/proc/$pid/exe" ]]; then
        executable="$(readlink "/proc/$pid/exe" 2>/dev/null || true)"
        executable="${executable% (deleted)}"
    else
        executable="$(ps -o comm= -p "$pid" 2>/dev/null || true)"
    fi
    case "$executable" in
        /*) printf '%s' "$executable" ;;
        *) return 1 ;;
    esac
}

# True when an executable path lives inside the release install. Used for
# candidates whose argv[0] says nothing about the file behind it — a daemon
# started from PATH has argv[0] `cua-driver` and could be any build on the
# machine, so its origin is what decides, not its name.
is_release_executable() {
    local executable="$1"
    shift
    local prefix
    for prefix in "$@"; do
        [[ -n "$prefix" ]] || continue
        case "$executable" in
            "$prefix"|"$prefix"/*) return 0 ;;
        esac
    done
    return 1
}

# Print the pids, one per line, of this uid's live processes that belong to the
# release install and whose subcommand is $3. An empty subcommand matches any
# invocation, and `?` selects the ones whose argument boundaries could not be
# recovered.
#
# $2 says how far argv[0] can be trusted:
#
#   path   argv[0] is itself a release install path, so matching it is proof
#          enough of what the process is.
#   name   argv[0] is the bare binary name, which any build on the machine can
#          have — a vendored `cua-driver` started from PATH is not ours to kill.
#          The executable behind the pid has to resolve into the install, and a
#          pid whose executable the host will not name is left alone.
#
# pgrep narrows the candidates; every decision after that reads the process
# itself, so a flag value that happens to be the word `serve` can never be
# mistaken for the subcommand.
collect_release_pids() {
    local uid="$1"
    local trust="$2"
    local subcommand="$3"
    shift 3
    local binary_regex pid executable
    local seen=" "
    for binary_regex in "$@"; do
        [[ -n "$binary_regex" ]] || continue
        while read -r pid; do
            [[ -n "$pid" ]] || continue
            case "$seen" in *" $pid "*) continue ;; esac
            seen="$seen$pid "
            daemon_pid_alive "$pid" || continue
            if [[ "$trust" == "name" ]]; then
                executable="$(daemon_pid_executable "$pid" || true)"
                [[ -n "$executable" ]] || continue
                is_release_executable "$executable" \
                    ${DAEMON_ORIGIN_PREFIXES[@]+"${DAEMON_ORIGIN_PREFIXES[@]}"} \
                    || continue
            fi
            if [[ -n "$subcommand" ]]; then
                [[ "$(daemon_pid_subcommand "$pid" || true)" == "$subcommand" ]] \
                    || continue
            fi
            printf '%s\n' "$pid"
        done < <(pgrep -U "$uid" -f "$(daemon_argv0_pattern "$binary_regex")" 2>/dev/null || true)
    done
}

# Both candidate passes for one subcommand, using the argv[0] regexes and
# origin prefixes the uninstall branch set up. Prints pids; empty output means
# there are none.
release_daemon_pids() {
    local uid="$1"
    local subcommand="$2"
    collect_release_pids "$uid" path "$subcommand" \
        ${DAEMON_ARGV0_REGEXES[@]+"${DAEMON_ARGV0_REGEXES[@]}"}
    collect_release_pids "$uid" name "$subcommand" \
        ${DAEMON_BARE_ARGV0_REGEXES[@]+"${DAEMON_BARE_ARGV0_REGEXES[@]}"}
}

# How long to let a supervisor respawn a daemon before re-checking. The
# release systemd unit restarts on failure after 2s, and a KeepAlive
# LaunchAgent is immediate, so a shorter wait would call a respawning daemon
# stopped.
DAEMON_RESPAWN_SETTLE_SECONDS=2.5

# Stop the running `cua-driver serve` daemon(s) belonging to the release
# install, using the argv[0] regexes and origin prefixes the uninstall branch
# set up.
#
# `pkill -x cua-driver` cannot be used: `-x` compares against the 15-character
# truncated `comm`, so it is unreliable for this binary name. Select by argv[0]
# instead, and require the `serve` subcommand so `cua-driver mcp` — a stdio
# child of a live MCP client, which exits with that client — is left alone.
#
# The uninstaller only ever touches this user's install, and it refuses to run
# as root, so every match is scoped to the invoking uid: on a shared Mac the
# same /Applications bundle can be running for someone else, and their daemon
# is neither ours to signal nor ours to wait for.
#
# Stopping a pid is not the same as stopping the daemon. The autostart teardown
# above is best-effort — `systemctl --user disable` and `launchctl unload`
# failures are swallowed so a half-configured host can still be uninstalled —
# so after the pids are confirmed gone the scan runs again: a supervisor that
# survived teardown puts a NEW pid there, and reporting the old one's death as
# success would be a lie.
#
# Returns 0 only when a re-scan finds nothing left, 1 when nothing matched at
# all, 2 when pgrep or ps is unavailable, 3 when a daemon survived SIGKILL, and
# 4 when something keeps starting new ones.
#
# ps is as required as pgrep: it is what resolves a candidate pid to the
# command line the subcommand decision needs, to the executable the origin
# check needs, and to the state that turns "signalled" into "confirmed gone".
# They ship together on every supported host, so demanding both costs nothing
# real.
stop_release_serve_daemons() {
    command -v pgrep >/dev/null 2>&1 || return 2
    command -v ps >/dev/null 2>&1 || return 2
    local uid
    uid="$(id -u)"
    local round=0
    local stopped_any=0
    local pid waited
    while [[ "$round" -lt 3 ]]; do
        local -a pids=()
        while read -r pid; do
            [[ -n "$pid" ]] || continue
            pids+=("$pid")
        done < <(release_daemon_pids "$uid" serve)
        if [[ "${#pids[@]}" -eq 0 ]]; then
            if [[ "$stopped_any" == "1" ]]; then
                return 0
            fi
            return 1
        fi
        stopped_any=1

        for pid in "${pids[@]}"; do
            kill -TERM "$pid" 2>/dev/null || true
        done

        # SIGTERM lets the daemon close its listening socket and tear down the
        # agent-cursor overlay. Escalate only for whatever is still alive after
        # a short grace period, so a wedged daemon cannot outlive the uninstall.
        waited=0
        while [[ "$waited" -lt 10 ]]; do
            daemon_pids_alive "${pids[@]}" || break
            # Fractional sleep is a BSD/GNU extension; fall back to whole
            # seconds rather than letting `set -e` abort the uninstall on a
            # host without it.
            sleep 0.1 2>/dev/null || sleep 1 || true
            waited=$((waited + 1))
        done
        if daemon_pids_alive "${pids[@]}"; then
            for pid in "${pids[@]}"; do
                kill -KILL "$pid" 2>/dev/null || true
            done
            # SIGKILL is not a guarantee — an uninterruptible wait, or a
            # process this uid may match but not signal, both survive it. The
            # whole point of this phase is that no daemon outlives the
            # uninstall, so confirm it instead of assuming it; the caller turns
            # a survivor into a warning, not a success.
            waited=0
            while [[ "$waited" -lt 5 ]]; do
                daemon_pids_alive "${pids[@]}" || break
                sleep 0.1 2>/dev/null || sleep 1 || true
                waited=$((waited + 1))
            done
            if daemon_pids_alive "${pids[@]}"; then
                return 3
            fi
        fi

        # Give a surviving supervisor time to put a new daemon there, then let
        # the loop find it.
        sleep "$DAEMON_RESPAWN_SETTLE_SECONDS" 2>/dev/null || sleep 3 || true
        round=$((round + 1))
    done
    return 4
}

# Report — never kill — non-`serve` processes still running from the release
# install paths. Those are `cua-driver mcp` children of a live MCP client
# (Claude Code, Codex); killing them surfaces to the user as a client-side
# transport error instead of a clean shutdown, and they exit with their client.
# Returns 0 when at least one such process is alive.
report_surviving_release_processes() {
    command -v pgrep >/dev/null 2>&1 || return 1
    command -v ps >/dev/null 2>&1 || return 1
    local uid
    uid="$(id -u)"
    [[ -n "$(release_daemon_pids "$uid" "")" ]]
}

# Report — never kill — processes whose subcommand could not be resolved
# because the host does not expose real argument boundaries (macOS). Saying so
# is the honest outcome: the alternative is guessing, which either kills an MCP
# child or silently leaves the daemon this uninstaller is meant to stop.
# Returns 0 when at least one such process is alive.
report_undetermined_release_processes() {
    command -v pgrep >/dev/null 2>&1 || return 1
    command -v ps >/dev/null 2>&1 || return 1
    local uid
    uid="$(id -u)"
    [[ -n "$(release_daemon_pids "$uid" "?")" ]]
}

reject_root_invocation() {
    local effective_uid="$1"
    if [[ "$effective_uid" == "0" ]]; then
        printf 'error: do not run the Cua Driver uninstaller with sudo; run it as the login user so Computer History is purged from the correct home directory and native credential store. The script elevates only protected app removal when needed.\n' >&2
        return 77
    fi
}

# Narrow source-only seam for the synthetic uninstall fixture. Production
# execution never sets this variable and continues through the full script.
if [[ "${CUA_DRIVER_UNINSTALL_TEST_SOURCE_ONLY:-0}" == "1" ]]; then
    return 0
fi

if ! reject_root_invocation "$(id -u)"; then
    exit 77
fi

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
    # install-local.sh --autostart registers
    # ~/.config/systemd/user/cua-driver.service. Stop + disable + remove it,
    # plus the pre-rename cua-driver-rs.service when present, so neither daemon
    # comes back at next logon.
    # systemctl --user no-ops gracefully on a non-systemd host.
    if [[ "$OS" == "Linux" ]]; then
        FOUND_SYSTEMD_USER_UNIT=0
        for SYSTEMD_USER_UNIT_PATH in "$SYSTEMD_USER_UNIT" "$LEGACY_SYSTEMD_USER_UNIT"; do
            if [[ -f "$SYSTEMD_USER_UNIT_PATH" ]]; then
                FOUND_SYSTEMD_USER_UNIT=1
                SYSTEMD_USER_UNIT_NAME="${SYSTEMD_USER_UNIT_PATH##*/}"
                if command -v systemctl >/dev/null 2>&1; then
                    systemctl --user stop "$SYSTEMD_USER_UNIT_NAME" 2>/dev/null || true
                    systemctl --user disable "$SYSTEMD_USER_UNIT_NAME" 2>/dev/null || true
                    log "stopped + disabled systemd --user unit $SYSTEMD_USER_UNIT_NAME"
                fi
                rm -f "$SYSTEMD_USER_UNIT_PATH"
                log "removed $SYSTEMD_USER_UNIT_PATH"
            fi
        done
        if [[ "$FOUND_SYSTEMD_USER_UNIT" == "0" ]]; then
            log "no current or legacy systemd --user unit found (skipping)"
        elif command -v systemctl >/dev/null 2>&1; then
            systemctl --user daemon-reload 2>/dev/null || true
        fi
    fi

    # --- Autostart (macOS LaunchAgent) ---
    # install-local.sh --autostart on macOS registers
    # ~/Library/LaunchAgents/com.trycua.cua-driver.plist. Unload (so the
    # running daemon stops) + remove it, plus the pre-rename
    # com.trycua.cua-driver-rs.plist when present.
    if [[ "$OS" == "Darwin" ]]; then
        FOUND_LAUNCHAGENT_PLIST=0
        for LAUNCHAGENT_PLIST_PATH in "$LAUNCHAGENT_PLIST" "$LEGACY_LAUNCHAGENT_PLIST"; do
            if [[ -f "$LAUNCHAGENT_PLIST_PATH" ]]; then
                FOUND_LAUNCHAGENT_PLIST=1
                launchctl unload "$LAUNCHAGENT_PLIST_PATH" 2>/dev/null || true
                rm -f "$LAUNCHAGENT_PLIST_PATH"
                log "removed LaunchAgent $LAUNCHAGENT_PLIST_PATH"
            fi
        done
        if [[ "$FOUND_LAUNCHAGENT_PLIST" == "0" ]]; then
            log "no current or legacy LaunchAgent found (skipping)"
        fi
    fi

    DAEMON_STOP_FAILED=0

    # --- Running daemon ---
    # Unix keeps an unlinked binary mapped, so removing the bundle and the
    # runtime payloads below leaves a running `cua-driver serve` alive on its
    # deleted path, still holding its listening socket, its accessibility and
    # screen-capture connections, and the agent-cursor overlay. uninstall.ps1
    # already stops every cua-driver process on Windows, where the open handles
    # make the removal itself fail — on Unix the same leftover is silent.
    #
    # Placement: after the autostart teardown above, because a KeepAlive
    # LaunchAgent or a Restart=on-failure systemd unit would respawn whatever
    # we stop; and before the history purge, the TCC reset, and the bundle
    # removal, so `--purge` takes the exclusive writer lease uncontended and
    # the com.trycua.driver grants are not revoked underneath a live process
    # signed with that identity.
    #
    # Gated on the Rust marker like every other shared-path action: on a
    # Swift-only Mac the bundle path and the CLI symlink resolve to the retired
    # Swift driver, whose daemon is not ours to stop.
    if [[ "$RUST_INSTALL_PRESENT" == "1" ]]; then
        DAEMON_ARGV0_REGEXES=(
            "$(escape_path_regex "$USER_BIN_LINK")"
            "$(escape_path_regex "$APP_BUNDLE/Contents/MacOS/cua-driver")"
            "$(escape_path_regex "$LEGACY_APP_BUNDLE/Contents/MacOS/cua-driver")"
            "$(escape_path_regex "$PACKAGES_DIR/current/cua-driver")"
            "$(escape_path_regex "$LEGACY_HOME_DIR/packages/current/cua-driver")"
            # A daemon the runtime started for itself carries the realpath of a
            # release in argv[0], because it resolves its own executable
            # through the packages/current symlink. Match that namespace as a
            # pattern rather than enumerating what is on disk: the installer
            # prunes old release directories, so a daemon that survived an
            # upgrade is running from a path that no longer exists — exactly
            # the unlinked-binary case this phase is here for.
            "$(escape_path_regex "$PACKAGES_DIR")/releases/[^[:space:]/]+/cua-driver"
            "$(escape_path_regex "$LEGACY_HOME_DIR/packages")/releases/[^[:space:]/]+/cua-driver"
        )
        # Started from a shell with the install dir on PATH: the shell passes
        # the word as typed, so argv[0] is the bare binary name and says
        # nothing about which build is behind it. Any vendored or hand-built
        # `cua-driver` on this machine has the same argv[0], so these
        # candidates are kept only when the executable itself resolves into the
        # install below. (`cua-driver-local` cannot match either way — the
        # pattern requires a separator right after the name.)
        DAEMON_BARE_ARGV0_REGEXES=("cua-driver")
        DAEMON_ORIGIN_PREFIXES=(
            "$APP_BUNDLE"
            "$LEGACY_APP_BUNDLE"
            "$HOME_DIR"
            "$LEGACY_HOME_DIR"
        )
        DAEMON_STOP_STATUS=0
        stop_release_serve_daemons || DAEMON_STOP_STATUS=$?
        case "$DAEMON_STOP_STATUS" in
            0) log "stopped the running cua-driver serve daemon" ;;
            2) log "pgrep/ps not found; a running cua-driver serve daemon (if any) has to be stopped by hand" ;;
            3)
                DAEMON_STOP_FAILED=1
                printf 'daemon_stop_incomplete: a cua-driver serve daemon survived SIGTERM and SIGKILL and is still running; stop it by hand before reinstalling.\n' >&2
                log "warning: could not stop the running cua-driver serve daemon (see stderr)"
                ;;
            4)
                DAEMON_STOP_FAILED=1
                printf 'daemon_stop_incomplete: a cua-driver serve daemon keeps being restarted after it is stopped, so an autostart supervisor is still active; check "systemctl --user list-units cua-driver*" or "launchctl list | grep cua-driver" and remove it before reinstalling.\n' >&2
                log "warning: a supervisor keeps restarting the cua-driver serve daemon (see stderr)"
                ;;
            *) log "no running cua-driver serve daemon (skipping)" ;;
        esac
        if report_undetermined_release_processes; then
            DAEMON_STOP_FAILED=1
            printf 'daemon_stop_undetermined: a running cua-driver process has an argument containing whitespace, and this host does not expose real argument boundaries to a shell, so it could not be told apart from an MCP client child; it was left alone. Inspect it with ps and stop it by hand if it is a serve daemon.\n' >&2
            log "warning: left a cua-driver process alone because its subcommand could not be resolved (see stderr)"
        fi
        if report_surviving_release_processes; then
            log "note: other cua-driver processes are still running; \`cua-driver mcp\` runs as a stdio child of your MCP client and exits when that client closes"
        fi
    else
        log "no Rust install marker; leaving any running cua-driver process untouched"
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
    # TCC grants were already revoked above, before the app was removed, so
    # the reset could still resolve the bundle id through LaunchServices.
    #
    # Everything on disk is gone either way; what a failed daemon stop changes
    # is whether "uninstalled" is the whole truth. Say so in the closing line
    # and exit non-zero, so a script that chains a reinstall after this one
    # does not proceed against a live daemon holding the old socket.
    UNINSTALL_HEADLINE="cua-driver uninstalled."
    if [[ "$DAEMON_STOP_FAILED" == "1" ]]; then
        UNINSTALL_HEADLINE="cua-driver uninstalled, but a cua-driver process is still running (see the daemon_stop_ line above)."
    fi
    if [[ "$OS" == "Darwin" ]]; then
        echo ""
        echo "$UNINSTALL_HEADLINE"
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
        echo "$UNINSTALL_HEADLINE"
        if [[ "$PURGE_DATA" == "0" ]]; then
            cat << 'TELEMETRYUNMSG'

Telemetry identity and preference were preserved for a future reinstall.
To delete them too, re-run with --purge:

  /bin/bash -c "$(curl -fsSL https://cua.ai/driver/uninstall.sh)" -- --purge
TELEMETRYUNMSG
        fi
    fi
    if [[ "$DAEMON_STOP_FAILED" == "1" ]]; then
        exit 1
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
