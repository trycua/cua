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
# macOS-only, no PowerShell counterpart:
#
#   cua_driver_release_processes      — every process of the current user that
#                                       runs a com.trycua.driver bundle, wherever
#                                       the app lives and whatever its version
#   cua_driver_stop_release_processes — stop those: graceful, then TERM, then KILL
#   unregister_other_cua_driver_copies — make LaunchServices forget every other
#                                       copy of the app (never deletes files)
#
# This script is sourced (not exec'd) by the Rust install helpers:
#
#   * _install-rust.sh           (production Rust delegate)
#   * _install-local-rust.sh     (dev Rust installer)
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
#   1. macOS: `launchctl unload <plist>` on the Rust LaunchAgent
#      (com.trycua.cua-driver-rs.plist). Unload
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
#   The Rust binary execs as `cua-driver`. One `pkill -x cua-driver`
#   stops it. `cua-driver-uia` is a Windows-only helper (see
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
                # Rust LaunchAgent plist.
                plist="$HOME/Library/LaunchAgents/com.trycua.cua-driver-rs.plist"
                if [ -f "$plist" ]; then
                    if [ -f "$plist" ]; then
                        launchctl unload "$plist" >/dev/null 2>&1 || true
                    fi
                fi
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

        if [ "$(uname -s 2>/dev/null || echo unknown)" = "Darwin" ]; then
            # macOS: stop what we can PROVE is Cua Driver (bundle id
            # com.trycua.driver, any path, any version) instead of every
            # process that happens to be named `cua-driver`. Another app can
            # ship its own helper with that name; it is not ours to kill.
            cua_driver_stop_release_processes
        elif command -v pkill >/dev/null 2>&1; then
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
    if [ "$(uname -s 2>/dev/null || echo unknown)" = "Darwin" ]; then
        local mac_survivors
        mac_survivors=$(cua_driver_release_processes | awk -F '\t' '{ printf "%s%s", sep, $1; sep = ", " }')
        [ -n "$mac_survivors" ] || return 0
        printf 'Note: Cua Driver process(es) still running after the stop attempt (pid: %s).\n' "$mac_survivors"
        printf '      They ignored SIGTERM and SIGKILL. Log out and back in, or run\n'
        printf '      `kill -9 <pid>` yourself, then re-run this installer.\n'
        return 0
    fi

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

# ---------------------------------------------------------------------------
# macOS: find and stop every Cua Driver process, wherever its app lives
# ---------------------------------------------------------------------------
#
# A Mac can hold several copies of CuaDriver.app (an old release in ~/Downloads,
# one in the Trash, a benchmark folder) and `open -a CuaDriver` may start any of
# them. A daemon from such a copy is not the file we are about to replace, so
# neither the pid file nor the install path finds it. We therefore identify
# processes by what they are: an executable named cua-driver inside an app
# bundle whose CFBundleIdentifier is com.trycua.driver, owned by this user.
# Other users' processes (ps -U <uid>) and other apps (a different bundle id,
# CuaDriverLocal's com.trycua.driver.local included) are never touched.

CUA_DRIVER_RELEASE_BUNDLE_ID="com.trycua.driver"

# Value of a top-level string key in an Info.plist. PlistBuddy reads binary and
# XML plists; the awk fallback reads XML plists, so test fixtures and hosts
# without PlistBuddy work too.
cua_driver_plist_string() {
    local plist="$1" key="$2" value=""
    [ -f "$plist" ] || return 1
    if [ -x /usr/libexec/PlistBuddy ]; then
        value="$(/usr/libexec/PlistBuddy -c "Print :$key" "$plist" 2>/dev/null)" || value=""
    fi
    if [ -z "$value" ]; then
        value="$(awk -v key="$key" '
            function text(line) { sub(".*<string>", "", line); sub("</string>.*", "", line); return line }
            index($0, "<key>" key "</key>") {
                if ($0 ~ /<string>/) { print text($0); exit }
                want = 1; next
            }
            want && /<string>/ { print text($0); exit }
            want && /<key>/ { exit }
        ' "$plist" 2>/dev/null)" || value=""
    fi
    [ -n "$value" ] || return 1
    printf '%s' "$value"
}

# The .app directory when "$1" (a process command line) runs a cua-driver
# executable inside an app bundle. Paths with spaces are fine: we cut at the
# fixed /Contents/MacOS/cua-driver suffix, not at the first space.
cua_driver_bundle_of_command() {
    local command_line="$1" bundle
    case "$command_line" in
        */Contents/MacOS/cua-driver|*/Contents/MacOS/cua-driver\ *) ;;
        *) return 1 ;;
    esac
    bundle="${command_line%%/Contents/MacOS/cua-driver*}"
    case "$bundle" in
        /*.app) printf '%s' "$bundle" ;;
        *) return 1 ;;
    esac
}

# True when the bundle is a Cua Driver release app. The bundle's own Info.plist
# decides. If it cannot be read (the app was deleted while its daemon kept
# running) only the stock names CuaDriver.app / CuaDriver-<version>.app count.
cua_driver_bundle_is_release() {
    local bundle="$1" id=""
    id="$(cua_driver_plist_string "$bundle/Contents/Info.plist" CFBundleIdentifier)" || id=""
    if [ -n "$id" ]; then
        [ "$id" = "$CUA_DRIVER_RELEASE_BUNDLE_ID" ]
        return
    fi
    case "${bundle##*/}" in
        CuaDriver.app|CuaDriver-*.app|"CuaDriver "*.app) return 0 ;;
        *) return 1 ;;
    esac
}

# This shell and every parent: an installer started by `cua-driver update` or by
# an agent that runs under a cua-driver process must not stop its own caller.
cua_driver_ancestor_pids() {
    local pid="$$" depth=0 parent
    while [ -n "$pid" ] && [ "$pid" -gt 1 ] 2>/dev/null && [ "$depth" -lt 32 ]; do
        printf '%s\n' "$pid"
        parent="$(ps -o ppid= -p "$pid" 2>/dev/null | tr -d '[:space:]')" || break
        [ -n "$parent" ] || break
        pid="$parent"
        depth=$((depth + 1))
    done
}

# One "<pid><TAB><bundle dir>" line per running Cua Driver process of this user.
cua_driver_release_processes() {
    local uid ancestors line pid command_line bundle
    uid="$(id -u 2>/dev/null)" || return 0
    command -v ps >/dev/null 2>&1 || return 0
    ancestors="$(cua_driver_ancestor_pids)"
    ps -U "$uid" -ww -o pid= -o command= 2>/dev/null | while IFS= read -r line; do
        line="${line#"${line%%[![:space:]]*}"}"
        pid="${line%%[[:space:]]*}"
        case "$pid" in ''|*[!0-9]*) continue ;; esac
        command_line="${line#"$pid"}"
        command_line="${command_line#"${command_line%%[![:space:]]*}"}"
        printf '%s\n' "$ancestors" | grep -qx "$pid" && continue
        bundle="$(cua_driver_bundle_of_command "$command_line")" || continue
        cua_driver_bundle_is_release "$bundle" || continue
        printf '%s\t%s\n' "$pid" "$bundle"
    done
}

# The bundle dir if "$1" is STILL a Cua Driver process (guards against pid reuse
# between listing and signalling).
cua_driver_pid_release_bundle() {
    local command_line bundle
    command_line="$(ps -ww -o command= -p "$1" 2>/dev/null)" || return 1
    command_line="${command_line#"${command_line%%[![:space:]]*}"}"
    bundle="$(cua_driver_bundle_of_command "$command_line")" || return 1
    cua_driver_bundle_is_release "$bundle" || return 1
    printf '%s' "$bundle"
}

# Wait up to $2 seconds for pid $1 to exit.
cua_driver_wait_gone() {
    local pid="$1" tenths=$(( ${2:-5} * 10 ))
    while kill -0 "$pid" 2>/dev/null; do
        [ "$tenths" -gt 0 ] || return 1
        sleep 0.1 2>/dev/null || sleep 1
        tenths=$((tenths - 1))
    done
    return 0
}

# Run a command, giving up (and killing it) after $1 seconds. Output is dropped.
cua_driver_run_bounded() {
    local limit="$1" child tenths
    shift
    "$@" >/dev/null 2>&1 &
    child=$!
    tenths=$((limit * 10))
    while kill -0 "$child" 2>/dev/null && [ "$tenths" -gt 0 ]; do
        sleep 0.1 2>/dev/null || sleep 1
        tenths=$((tenths - 1))
    done
    kill "$child" 2>/dev/null || true
    wait "$child" 2>/dev/null || true
}

# Stop every Cua Driver process of this user, wherever its app lives. Order:
# the daemon's own `stop` request (clean shutdown), then SIGTERM, then SIGKILL,
# each given CUA_DRIVER_STOP_TIMEOUT seconds (default 5). Prints one line per
# process saying how it ended. Returns 1 if any process survived.
cua_driver_stop_release_processes() {
    local timeout="${CUA_DRIVER_STOP_TIMEOUT:-5}" list pid bundle version label how helper tab failed=0
    tab="$(printf '\t')"
    list="$(cua_driver_release_processes)"
    [ -n "$list" ] || return 0
    while IFS="$tab" read -r pid bundle; do
        [ -n "$pid" ] || continue
        version="$(cua_driver_plist_string "$bundle/Contents/Info.plist" CFBundleShortVersionString)" || version=""
        label="pid $pid, ${version:+version $version, }$bundle"
        how=""

        # 1. Ask the daemon to shut down through its own bundle's CLI. Old
        #    releases do not know --expected-pid and simply fail; that is fine.
        helper="$bundle/Contents/MacOS/cua-driver"
        if [ -x "$helper" ] && cua_driver_pid_release_bundle "$pid" >/dev/null; then
            cua_driver_run_bounded "$timeout" "$helper" --expected-pid "$pid" stop
            if cua_driver_wait_gone "$pid" 1; then
                how="a graceful stop request"
            fi
        fi
        # 2. SIGTERM, re-checking the pid still belongs to Cua Driver.
        if [ -z "$how" ] && cua_driver_pid_release_bundle "$pid" >/dev/null; then
            kill -TERM "$pid" 2>/dev/null || true
            if cua_driver_wait_gone "$pid" "$timeout"; then
                how="SIGTERM"
            fi
        fi
        # 3. SIGKILL.
        if [ -z "$how" ] && cua_driver_pid_release_bundle "$pid" >/dev/null; then
            kill -KILL "$pid" 2>/dev/null || true
            if cua_driver_wait_gone "$pid" 2; then
                how="SIGKILL after it ignored SIGTERM for ${timeout}s"
            fi
        fi
        if [ -z "$how" ] && ! kill -0 "$pid" 2>/dev/null; then
            how="its own exit"
        fi
        if [ -n "$how" ]; then
            printf '==> stopped Cua Driver (%s) with %s\n' "$label" "$how"
        else
            printf '==> could not stop Cua Driver (%s); it is still running\n' "$label" >&2
            failed=1
        fi
    done <<EOF
$list
EOF
    return "$failed"
}

# ---------------------------------------------------------------------------
# macOS: make LaunchServices forget every other copy of the app
# ---------------------------------------------------------------------------
#
# `open -a CuaDriver`, a bundle-id lookup and System Settings' permission rows
# all resolve to whichever registered copy macOS likes best, which is how an old
# copy in the Trash became the "CuaDriver-0.34.0" row. After an install only the
# installed bundle should stay registered. `lsregister -u` removes the
# registration only; no file is deleted.

# Where lsregister lives. CUA_DRIVER_LSREGISTER overrides it (tests).
cua_driver_lsregister_path() {
    printf '%s' "${CUA_DRIVER_LSREGISTER:-/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister}"
}

# Paths of every bundle with the given id that LaunchServices (lsregister -dump)
# or Spotlight (mdfind) knows about, one per line, sorted and unique. The dump
# is slow on a busy Mac (tens of seconds), so it is capped by
# CUA_DRIVER_LSREGISTER_TIMEOUT seconds (default 120).
cua_driver_known_bundle_paths() {
    local bundle_id="${1:-$CUA_DRIVER_RELEASE_BUNDLE_ID}" lsregister dump pid waited=0 limit
    lsregister="$(cua_driver_lsregister_path)"
    limit="${CUA_DRIVER_LSREGISTER_TIMEOUT:-120}"
    {
        if [ -x "$lsregister" ]; then
            dump="$(mktemp -t cua-lsdump.XXXXXX 2>/dev/null)" || dump=""
            if [ -n "$dump" ]; then
                "$lsregister" -dump >"$dump" 2>/dev/null &
                pid=$!
                while kill -0 "$pid" 2>/dev/null; do
                    if [ "$waited" -ge "$limit" ]; then
                        kill "$pid" 2>/dev/null || true
                        break
                    fi
                    sleep 1
                    waited=$((waited + 1))
                done
                wait "$pid" 2>/dev/null || true
                # A dump is a series of blocks split by a line of dashes; each
                # block has "path:" and "identifier:" lines.
                awk -v id="$bundle_id" '
                    function flush() { if (path != "" && matches) print path; path = ""; matches = 0 }
                    /^----------/ { flush(); next }
                    /^path:[ \t]/ {
                        p = $0; sub(/^path:[ \t]+/, "", p); sub(/[ \t]+\(0x[0-9a-fA-F]+\)$/, "", p)
                        path = p; next
                    }
                    /^identifier:[ \t]/ {
                        i = $0; sub(/^identifier:[ \t]+/, "", i); sub(/[ \t]+$/, "", i)
                        matches = (i == id); next
                    }
                    END { flush() }
                ' "$dump"
                rm -f "$dump"
            fi
        fi
        if command -v mdfind >/dev/null 2>&1; then
            mdfind "kMDItemCFBundleIdentifier == '$bundle_id'" 2>/dev/null || true
        fi
    } | sed '/^$/d' | sort -u
}

# unregister_other_cua_driver_copies <installed bundle dir>
# Unregisters every other com.trycua.driver bundle and prints one summary line.
# Best effort: never fails the install, never deletes anything.
unregister_other_cua_driver_copies() {
    local active="$1" lsregister active_real path real found=0 paths word
    lsregister="$(cua_driver_lsregister_path)"
    [ -x "$lsregister" ] || return 0
    active_real="$(cd "$active" 2>/dev/null && pwd -P)" || active_real="$active"
    paths="$(cua_driver_known_bundle_paths "$CUA_DRIVER_RELEASE_BUNDLE_ID")"
    [ -n "$paths" ] || return 0
    while IFS= read -r path; do
        [ -n "$path" ] || continue
        real="$path"
        if [ -d "$path" ]; then
            real="$(cd "$path" 2>/dev/null && pwd -P)" || real="$path"
        fi
        if [ "$path" = "$active" ] || [ "$real" = "$active_real" ]; then
            continue
        fi
        found=$((found + 1))
        if ! "$lsregister" -u "$path" >/dev/null 2>&1; then
            printf 'warning: could not unregister the old Cua Driver copy at %s\n' "$path" >&2
        fi
    done <<EOF
$paths
EOF
    [ "$found" -gt 0 ] || return 0
    word="copies"
    [ "$found" -ne 1 ] || word="copy"
    printf 'Found %s old Cua Driver %s; macOS will now only use %s\n' "$found" "$word" "$active"
    return 0
}
