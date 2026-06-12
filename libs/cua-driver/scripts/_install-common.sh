# _install-common.sh — shared helpers for the .sh installers.
#
# Bash counterpart to CuaDriverInstall.psm1. Both files have to stay in
# lockstep on the kill / probe behaviour so a Mac/Linux dev fix matches
# what install.ps1 does on Windows. Function names mirror the
# PowerShell side:
#
#   stop_cua_driver_daemons          ↔  Stop-CuaDriverDaemons
#   show_cua_driver_daemon_survivors ↔  Show-CuaDriverDaemonSurvivors
#
# This script is sourced (not exec'd) by the per-backend install
# helpers:
#
#   * _install-rust.sh           (production Rust delegate)
#   * _install-local-rust.sh     (dev Rust installer)
#   * _install-local-swift.sh    (dev Swift installer)
#
# Loaders: on-disk first when run from a checked-out tree, else fetched
# from GitHub raw via `curl` (mirrors the irm | iex path that
# install.ps1 uses to import the .psm1 over the network). The inline
# loader is duplicated in each consumer because `source` doesn't have a
# bootstrap function the way PowerShell's `Import-Module` does — kept
# minimal so the duplication cost stays low.
#
# Keep this file narrow on purpose: every byte gets curl'd on every
# `curl ... | bash` install on non-macOS hosts (where install.sh
# auto-delegates to the Rust path) AND on dev installs. Things that DO
# belong here: kill / wait / probe helpers that multiple consumers
# need. Things that DON'T: anything only one script uses (leave it
# inline there).
#
# Style:
#   * No `set -e` — sourced helpers shouldn't change the caller's shell
#     options. Caller scripts are already `set -euo pipefail`.
#   * Best-effort everywhere: every external command is suffixed with
#     `|| true` (or wrapped in a subshell) so a kill failure never
#     aborts the surrounding install.
#   * Bash 3.2 compatible (macOS default). No associative arrays, no
#     `[[ =~ ]]` patterns that need bash 4+.

# Best-effort kill of any running cua-driver daemons so the next
# `cua-driver` invocation starts the FRESH binary, not whatever's still
# in memory from the pre-upgrade install. Without this the previous
# daemon keeps running (and on macOS keeps holding TCC-attributed file
# handles) until the user logs out — which surfaces as "the bug I just
# fixed is still there" because the in-memory code is pre-fix.
#
# Layered escalation, in order of decreasing politeness:
#   1. macOS: `launchctl unload <plist>` on every LaunchAgent we know
#      we install (com.trycua.cua-driver-rs.plist for the Rust port,
#      com.trycua.cua_driver_daemon.plist for the Swift driver). Unload
#      is the documented way to stop a launchd-managed daemon — it
#      also clears the KeepAlive flag so launchd doesn't immediately
#      respawn the process we're about to kill. No-op (with stderr
#      suppressed) when the plist isn't installed.
#      Linux: `systemctl --user stop cua-driver-rs.service` for the
#      install-local --autostart path. Same shape — politely stop the
#      supervisor first so it doesn't restart the process.
#   2. `pkill -x cua-driver` as the backstop for processes that weren't
#      launchd/systemd-supervised (e.g. a manual `cua-driver serve`
#      from a dev shell, or `cua-driver mcp` spawned by an editor that
#      isn't aware we're swapping the binary out from under it).
#
# Daemon-name coverage:
#   The Rust binary and the Swift binary BOTH exec as `cua-driver` (same
#   product name — see build-app.sh `--identifier com.trycua.driver`
#   and _install-rust.sh `BINARY_NAME="cua-driver"`). One `pkill -x
#   cua-driver` covers both, so callers don't need a per-backend
#   variant — the kill is naturally symmetric across the Rust/Swift
#   split. `cua-driver-uia` is a Windows-only helper (see
#   libs/cua-driver/rust/crates/cua-driver-uia/) and never exists on
#   macOS/Linux, so no pkill for it here.
#
# Returns: always 0. Caller threads this through unconditionally; the
# behaviour is "kill what we can, never block the install".
stop_cua_driver_daemons() {
    printf '==> stopping any running cua-driver daemons before swap\n'

    # Wrap the whole thing in a subshell so any unexpected non-zero exit
    # (e.g. a `pkill` returning 1 when no process matches under a
    # `set -e` caller — we shouldn't be `set -e` here, but defence in
    # depth) never escapes.
    (
        case "$(uname -s 2>/dev/null || echo unknown)" in
            Darwin)
                # Both known LaunchAgent plists. `launchctl unload` is a
                # no-op-with-warning when the plist doesn't exist; swallow
                # stderr to keep the install log clean.
                local plist
                for plist in \
                    "$HOME/Library/LaunchAgents/com.trycua.cua-driver-rs.plist" \
                    "$HOME/Library/LaunchAgents/com.trycua.cua_driver_daemon.plist"
                do
                    if [ -f "$plist" ]; then
                        launchctl unload "$plist" >/dev/null 2>&1 || true
                    fi
                done
                ;;
            Linux)
                # systemctl --user is the only supported supervisor on
                # Linux today (install-local-rust.sh --autostart writes
                # ~/.config/systemd/user/cua-driver-rs.service). `command
                # -v` so we don't error on systemd-less hosts (musl
                # containers, NixOS without user services, etc.).
                if command -v systemctl >/dev/null 2>&1; then
                    systemctl --user stop cua-driver-rs.service >/dev/null 2>&1 || true
                fi
                ;;
            *)
                # Other Unixes (FreeBSD, etc.) — no supervisor we know
                # about, fall through to the pkill backstop.
                ;;
        esac

        # Best-effort: give the supervisor up to ~200ms to take the
        # daemon down before we resort to pkill. Same wait as the
        # PowerShell module after schtasks /End.
        sleep 0.2 2>/dev/null || sleep 1

        if command -v pkill >/dev/null 2>&1; then
            # `-x` = exact match on process name so we don't kill a
            # user's `cua-driver-rs-foo` test harness or any script
            # named "cua-driver-something". The Rust + Swift binaries
            # both exec as exactly `cua-driver` so a single pkill
            # covers both.
            pkill -x cua-driver >/dev/null 2>&1 || true
        fi
    ) || true

    return 0
}

# Print a yellow warning if cua-driver processes are still running after
# stop_cua_driver_daemons did its best. On macOS/Linux this almost never
# fires for processes the current user owns — `pkill` succeeds against
# user-owned processes without elevation — so when it DOES fire it
# usually means:
#
#   * Another user on the same host has their own cua-driver running
#     (multi-user dev box). Their daemon is fine; ours just won't see
#     the swap until they restart theirs.
#   * The daemon was spawned with a custom signal handler that ignores
#     SIGTERM (we don't ship such a handler — but a debugger session
#     could install one).
#   * Process is in uninterruptible state (D-state on Linux from a
#     stuck syscall — rare for a userspace daemon).
#
# Mirrors Show-CuaDriverDaemonSurvivors on the PowerShell side, but the
# Unix-side mitigation is different: there's no clean User Account
# Control / High-IL escalation analogue, so the hint is just `sudo
# pkill` (root can reach other users' processes).
#
# Idempotent — safe to call even when stop_cua_driver_daemons did
# clean up everything. Prints nothing in the common case.
show_cua_driver_daemon_survivors() {
    # `pgrep -x` matches the exact process name the same way `pkill -x`
    # did above, so the survivor list is exactly what the kill couldn't
    # reach (not random other cua-driver-* binaries).
    if ! command -v pgrep >/dev/null 2>&1; then
        # No pgrep, no way to check. Bail quietly — the install can
        # still succeed; worst case the user notices a stale binary
        # and reboots.
        return 0
    fi

    local survivor_pids
    survivor_pids=$(pgrep -x cua-driver 2>/dev/null || true)
    if [ -z "$survivor_pids" ]; then
        return 0
    fi

    # `tput` may not be available (CI, agent sandboxes without TERM);
    # guard so we don't crash before printing the actual warning. The
    # color is decoration — the text is the load-bearing part.
    local yellow normal
    yellow=$(tput setaf 3 2>/dev/null || true)
    normal=$(tput sgr0 2>/dev/null || true)

    # `wc -w` is portable across BSD and GNU; pgrep prints one pid per
    # line so we'd get the same count from `wc -l`, but `wc -w` is
    # robust to a missing trailing newline.
    local count
    count=$(printf '%s\n' "$survivor_pids" | wc -w | tr -d ' ')
    local pid_csv
    pid_csv=$(printf '%s\n' "$survivor_pids" | tr '\n' ',' | sed 's/,$//' | sed 's/,/, /g')

    printf '%sNote: %s cua-driver process(es) still running after best-effort kill (pid: %s).%s\n' \
        "$yellow" "$count" "$pid_csv" "$normal"
    printf '%s      They are probably owned by another user, or are ignoring SIGTERM.%s\n' \
        "$yellow" "$normal"
    printf '%s      To force-kill (needs root if owned by another user):%s\n' \
        "$yellow" "$normal"
    printf '%s        sudo pkill -9 -x cua-driver%s\n' \
        "$yellow" "$normal"
    printf '%s      Or log out and back in. Until they exit, the OLD binary keeps running.%s\n' \
        "$yellow" "$normal"
    return 0
}
