#!/bin/sh
# Cua uninstaller for macOS and Linux (Windows: uninstall.ps1). It removes what
# install.sh and the Cua Spaces app put on this computer.
#
#   curl -fsSL https://cua.ai/uninstall.sh | sh
#   curl -fsSL https://cua.ai/uninstall.sh | sh -s -- --dry-run
#   curl -fsSL https://cua.ai/uninstall.sh | sh -s -- --yes --keep-data
#   curl -fsSL https://cua.ai/uninstall.sh | sh -s -- --all
#
# By default it removes the Cua Spaces app, the cua CLI and its PATH line, the
# host service (`cua host remove --uninstalling`), your agents' Cua Spaces
# skills and MCP entries (`cua agents remove`: the `cua` server and the Cua
# Spaces skills; Cua Driver's `cua-driver` entries and other tools' stay),
# launch at login, your sign-in and device key (macOS Keychain, including the
# old run.cua.ai item), and Cua's settings, caches and logs. It never deletes a
# Space or its data: that needs --delete-spaces. Run `uninstall.sh --help` for
# the options.
set -eu

usage() {
    cat <<'EOF'
Cua uninstaller: removes the Cua Spaces app, the cua CLI, the host service,
Cua Spaces' agent skills and MCP entries, sign-in and Cua's settings. Spaces
and their data stay unless you ask for them to go. Cua Driver's skill and MCP
server in your agents, and other tools' entries, stay too (--all removes Cua
Driver's).

Options:
  --keep-data        keep ~/.cua (settings, the Spaces registry, caches, the
                     Cua Volume cache, device key); programs, services,
                     integrations and your sign-in are still removed
  --delete-spaces    ALSO delete the Spaces on this computer and their data
                     (Space disks, the local Cua Volume files, the Keyvault).
                     Running Spaces are listed and stopped first. Cannot be
                     undone; asks you to type "delete" unless --yes is given
  --all              also run the Cua Driver and Lume uninstallers, and remove
                     Cua Driver's skill and MCP server from your agents
                     (Lume keeps your VMs unless --delete-spaces is given too)
  --prefix DIR       also remove what install.sh --prefix DIR put there
                     (DIR/bin, DIR/Applications)
  --no-modify-path   never edit your shell profile
  --dry-run          print what would be removed, change nothing (it lists
                     the agent skills and MCP entries one by one)
  -y, --yes          accept every prompt (non-interactive)
  -h, --help         show this help

Environment:
  CUA_UNINSTALL_NONINTERACTIVE=1  never prompt (needs --yes to remove anything)
  CUA_HOME                        cua state dir (default ~/.cua)
EOF
}

# Test hooks: CUA_UNINSTALL_OS, CUA_UNINSTALL_DRIVER_URL, CUA_UNINSTALL_LUME_URL,
# CUA_UNINSTALL_ROOT (a folder standing in for / when looking at /Applications,
# /usr/local/bin, /opt and /etc).
DRIVER_URL="${CUA_UNINSTALL_DRIVER_URL:-https://cua.ai/driver/uninstall.sh}"
LUME_URL="${CUA_UNINSTALL_LUME_URL:-https://cua.ai/lume/uninstall.sh}"
# Written above a PATH edit by install.sh and by the Cua Spaces app.
MARKER="# Added by the cua installer"
# Settings and local state inside the cua home, removed unless --keep-data.
# Spaces and their data (sandboxes, vmm, images, volume/data, the Keyvault,
# trajectories, spaces.json) are not on this list: only --delete-spaces removes them.
DISPOSABLE="cache cache.json cua.sock keyvault.sock x.sock daemon.json daemon.starting fleet-gc.lock config config.toml env-tokens.json do_target.json host_consented host spacesd telemetry skills bin runtimes spaces-install-mode volume/cache"
# The sign-in goes with the uninstall even with --keep-data.
SESSION_FILES="credentials.json spaces-session.json spaces-credentials.json"

keep_data=0
delete_spaces=0
all_tools=0
prefix=""
no_modify_path=0
dry_run=0
assume_yes=0
tmp=""
failed=0
cua_bin=""

say() { printf '%s\n' "$*"; }
info() { printf 'cua-uninstall: %s\n' "$*"; }
warn() { printf 'cua-uninstall: warning: %s\n' "$*" >&2; }
die() {
    printf 'cua-uninstall: error: %s\n' "$*" >&2
    exit 1
}

cleanup() {
    if [ -n "$tmp" ] && [ -d "$tmp" ]; then
        rm -rf "$tmp"
    fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# ---------------------------------------------------------------- arguments

parse_args() {
    while [ $# -gt 0 ]; do
        case "$1" in
        --keep-data) keep_data=1 ;;
        --delete-spaces) delete_spaces=1 ;;
        --all) all_tools=1 ;;
        --prefix)
            [ $# -ge 2 ] || die "--prefix needs a directory"
            prefix="$2"
            shift
            ;;
        --prefix=*) prefix="${1#--prefix=}" ;;
        --no-modify-path) no_modify_path=1 ;;
        --dry-run) dry_run=1 ;;
        -y | --yes) assume_yes=1 ;;
        -h | --help)
            usage
            exit 0
            ;;
        *) die "unknown option: $1 (see --help)" ;;
        esac
        shift
    done
    if [ "$keep_data" = 1 ] && [ "$delete_spaces" = 1 ]; then
        die "--keep-data and --delete-spaces contradict each other"
    fi
    case "$prefix" in
    "" | /*) ;;
    *) prefix="$(pwd)/$prefix" ;;
    esac
    prefix="${prefix%/}"
}

# ---------------------------------------------------------------- helpers

has() { command -v "$1" >/dev/null 2>&1; }

# Whether a human can answer prompts (curl | sh keeps /dev/tty usable).
tty_ok() {
    [ "${CUA_UNINSTALL_NONINTERACTIVE:-}" != 1 ] || return 1
    [ -t 1 ] || [ -t 2 ] || return 1
    (: </dev/tty) 2>/dev/null
}

# confirm "question": 0 for yes. Default is no.
confirm() {
    [ "$assume_yes" = 1 ] && return 0
    tty_ok || return 1
    printf '%s [y/N] ' "$1" >/dev/tty
    answer=""
    read -r answer </dev/tty || answer=""
    case "$answer" in
    [yY] | [yY][eE][sS]) return 0 ;;
    *) return 1 ;;
    esac
}

# bounded SECONDS COMMAND...: runs the command with no input and kills it
# after SECONDS (macOS has no timeout(1)). Returns the command's status.
bounded() {
    secs="$1"
    shift
    "$@" </dev/null &
    pid=$!
    (
        elapsed=0
        while [ "$elapsed" -lt "$secs" ] && kill -0 "$pid" 2>/dev/null; do
            sleep 1
            elapsed=$((elapsed + 1))
        done
        if [ "$elapsed" -ge "$secs" ]; then kill "$pid" 2>/dev/null; fi
    ) >/dev/null 2>&1 &
    watchdog=$!
    status=0
    wait "$pid" || status=$?
    kill "$watchdog" 2>/dev/null || true
    wait "$watchdog" 2>/dev/null || true
    return "$status"
}

# A path this script may delete: absolute, not a system or home directory.
safe_path() {
    case "$1" in
    "" | / | //*) return 1 ;;
    /*) ;;
    *) return 1 ;;
    esac
    case "$1" in
    */../* | */.. | */./* | */.) return 1 ;;
    esac
    candidate="${1%/}"
    for guard in "$HOME" "$sysroot/Applications" "$sysroot/Library" "$sysroot/Users" "$sysroot/home" "$sysroot/usr" \
        "$sysroot/usr/local" "$sysroot/usr/local/bin" "$sysroot/usr/bin" "$sysroot/opt" "$sysroot/etc" "$sysroot/var" /tmp \
        "$HOME/Applications" "$HOME/Library" "$HOME/.local" "$HOME/.local/bin" "$HOME/.local/share" "$HOME/.config" "$HOME/.cache"; do
        [ "$candidate" = "${guard%/}" ] && return 1
    done
    return 0
}

# rm_path PATH: remove a file, link or folder when it exists. Says so when it
# cannot (a root-owned app) instead of stopping.
rm_path() {
    [ -e "$1" ] || [ -L "$1" ] || return 0
    if ! safe_path "$1"; then
        warn "not touching $1"
        return 0
    fi
    if [ "$dry_run" = 1 ]; then
        say "[dry-run] remove $1"
        return 0
    fi
    if [ -d "$1" ] && [ ! -L "$1" ]; then
        rm -rf -- "$1" 2>/dev/null || true
    else
        rm -f -- "$1" 2>/dev/null || true
    fi
    if [ -e "$1" ] || [ -L "$1" ]; then
        warn "could not remove $1 (try: sudo rm -rf \"$1\")"
        failed=1
    else
        info "removed $1"
    fi
}

# rmdir_quiet DIR: remove DIR only when it is empty.
rmdir_quiet() {
    [ "$dry_run" = 1 ] && return 0
    [ -d "$1" ] && [ ! -L "$1" ] || return 0
    rmdir "$1" 2>/dev/null || true
}

# run COMMAND...: runs it, or only prints it with --dry-run.
run() {
    if [ "$dry_run" = 1 ]; then
        say "[dry-run] $*"
    else
        "$@"
    fi
}

# run_cua LABEL SECONDS ARGS...: the cua CLI, bounded, without usage telemetry.
run_cua() {
    label="$1"
    secs="$2"
    shift 2
    if [ "$dry_run" = 1 ]; then
        say "[dry-run] cua $*"
        return 0
    fi
    info "$label"
    if ! bounded "$secs" env CUA_TELEMETRY=0 DO_NOT_TRACK=1 "$cua_bin" "$@"; then
        warn "'cua $*' did not finish; continuing"
    fi
}

detect_platform() {
    os="${CUA_UNINSTALL_OS:-$(uname -s)}"
    case "$os" in
    Darwin | darwin | macos) os=darwin ;;
    Linux | linux) os=linux ;;
    MINGW* | MSYS* | CYGWIN* | Windows_NT | windows)
        die "on Windows, run: irm https://cua.ai/uninstall.ps1 | iex"
        ;;
    *) die "unsupported OS: $os" ;;
    esac
    [ -n "${HOME:-}" ] && [ -d "$HOME" ] || die "HOME is not set to a folder"
    sysroot="${CUA_UNINSTALL_ROOT:-}"
    sysroot="${sysroot%/}"
    cua_home="${CUA_HOME:-$HOME/.cua}"
    data_home="${XDG_DATA_HOME:-$HOME/.local/share}"
    config_home="${XDG_CONFIG_HOME:-$HOME/.config}"
}

# ---------------------------------------------------------------- what is here

# is_cua_cli FILE: the file is a cua CLI that Cua put there: a link into the
# Cua Spaces app, or a program whose `--version` says `cua X.Y.Z`. A script
# with the same name (a pip install) is left alone.
is_cua_cli() {
    if [ -L "$1" ]; then
        case "$(readlink "$1" 2>/dev/null || true)" in
        *"Cua Spaces.app"* | *"/Cua Spaces/"*) return 0 ;;
        esac
    fi
    [ -f "$1" ] && [ -x "$1" ] || return 1
    version="$(bounded 10 "$1" --version 2>/dev/null | head -n 1 || true)"
    case "$version" in
    "cua "[0-9]*.[0-9]*.[0-9]*) return 0 ;;
    *) return 1 ;;
    esac
}

# is_cua_app APP: an app folder named Cua Spaces.app that is Cua's (a bundle
# with another identifier is somebody else's).
is_cua_app() {
    id=""
    if has defaults && [ -f "$1/Contents/Info.plist" ]; then
        id="$(defaults read "$1/Contents/Info" CFBundleIdentifier 2>/dev/null || true)"
    fi
    case "$id" in
    "" | com.trycua.* | ai.cua.*) return 0 ;;
    *) return 1 ;;
    esac
}

cli_candidates() {
    {
        [ -z "$prefix" ] || printf '%s\n' "$prefix/bin/cua"
        printf '%s\n' "$HOME/.local/bin/cua" "$sysroot/usr/local/bin/cua"
    } | awk '!seen[$0]++'
}

app_candidates() {
    case "$os" in
    darwin)
        {
            [ -z "$prefix" ] || printf '%s\n' "$prefix/Applications/Cua Spaces.app"
            printf '%s\n' "$sysroot/Applications/Cua Spaces.app" "$HOME/Applications/Cua Spaces.app"
        } | awk '!seen[$0]++'
        ;;
    linux)
        {
            [ -z "$prefix" ] || printf '%s\n' "$prefix/bin/cua-spaces"
            printf '%s\n' "$HOME/.local/bin/cua-spaces" "$sysroot/usr/local/bin/cua-spaces"
        } | awk '!seen[$0]++'
        ;;
    esac
}

deb_installed() {
    [ "$os" = linux ] && has dpkg-query &&
        dpkg-query -W -f='${Status}' cua-spaces 2>/dev/null | grep -q 'install ok installed'
}

# find_cua: the cua CLI to run the removal steps with.
find_cua() {
    cua_bin=""
    for candidate in "$(command -v cua 2>/dev/null || true)" "$HOME/.local/bin/cua" "$sysroot/usr/local/bin/cua" \
        "$sysroot/Applications/Cua Spaces.app/Contents/MacOS/cua" "$HOME/Applications/Cua Spaces.app/Contents/MacOS/cua" \
        "$sysroot/opt/Cua Spaces/resources/native/cua" "${prefix:+$prefix/bin/cua}" "${prefix:+$prefix/Applications/Cua Spaces.app/Contents/MacOS/cua}"; do
        [ -n "$candidate" ] && [ -x "$candidate" ] || continue
        version="$(bounded 10 "$candidate" --version 2>/dev/null | head -n 1 || true)"
        case "$version" in
        "cua "[0-9]*.[0-9]*.[0-9]*)
            cua_bin="$candidate"
            return 0
            ;;
        esac
    done
    return 0
}

host_service_files() {
    case "$os" in
    darwin)
        printf '%s\n' "$HOME/Library/LaunchAgents/com.trycua.spacesd.host.plist" \
            "$HOME/Library/LaunchAgents/com.trycua.env-driver.host.plist" "$HOME/.cua/helper/service.json"
        ;;
    linux)
        printf '%s\n' "$config_home/systemd/user/cua-spacesd-host.service" \
            "$config_home/systemd/user/cua-env-driver-host.service" \
            "$sysroot/etc/systemd/system/cua-spacesd-host.service" "$sysroot/etc/systemd/system/cua-env-driver-host.service"
        ;;
    esac
}

# cua homes: the main one and one per environment (~/.cua/envs/staging).
cua_homes() {
    [ -d "$cua_home" ] || return 0
    printf '%s\n' "$cua_home"
    for env_home in "$cua_home"/envs/*; do
        [ -d "$env_home" ] && printf '%s\n' "$env_home"
    done
    return 0
}

library_dirs_removed_always() {
    [ "$os" = darwin ] || return 0
    printf '%s\n' "$HOME/Library/Caches/com.trycua.spaces.macos" \
        "$HOME/Library/Caches/com.trycua.cua-spaces" \
        "$HOME/Library/Saved Application State/com.trycua.spaces.macos.savedState" \
        "$HOME/Library/HTTPStorages/com.trycua.spaces.macos" \
        "$HOME/Library/HTTPStorages/com.trycua.spaces.macos.binarycookies" \
        "$HOME/Library/WebKit/com.trycua.spaces.macos"
}

library_dirs_removed_unless_keep() {
    case "$os" in
    darwin)
        printf '%s\n' "$HOME/Library/Application Support/com.trycua.spaces.macos" \
            "$HOME/Library/Preferences/com.trycua.spaces.macos.plist"
        ;;
    linux)
        printf '%s\n' "$config_home/Cua Spaces"
        ;;
    esac
}

kc_has() {
    has security && bounded 20 security find-generic-password -s "$1" >/dev/null 2>&1
}

# ---------------------------------------------------------------- survey

found=0
found_lines=""
add_found() {
    found=1
    found_lines="$found_lines  $1
"
}

survey() {
    while IFS= read -r app; do
        [ -n "$app" ] || continue
        if { [ -e "$app" ] || [ -L "$app" ]; } && { [ "$os" != darwin ] || is_cua_app "$app"; }; then
            add_found "Cua Spaces app: $app"
        fi
    done <<EOF
$(app_candidates)
EOF
    if deb_installed; then
        add_found "Cua Spaces package: cua-spaces (.deb)"
    fi
    while IFS= read -r cli; do
        [ -n "$cli" ] || continue
        if { [ -e "$cli" ] || [ -L "$cli" ]; } && is_cua_cli "$cli"; then
            add_found "cua CLI: $cli"
        fi
    done <<EOF
$(cli_candidates)
EOF
    while IFS= read -r svc; do
        [ -n "$svc" ] || continue
        if [ -e "$svc" ]; then
            add_found "host service: $svc"
        fi
    done <<EOF
$(host_service_files)
EOF
    if [ -d "$cua_home" ]; then
        add_found "cua state: $cua_home"
    fi
    if [ "$os" = darwin ] && { kc_has run.cua.ai || kc_has staging.run.cua.ai; }; then
        add_found "macOS Keychain items: run.cua.ai (sign-in, device key)"
    fi
    while IFS= read -r lib; do
        [ -n "$lib" ] || continue
        if [ -e "$lib" ]; then
            add_found "app settings and caches: $lib"
        fi
    done <<EOF
$(library_dirs_removed_always)
$(library_dirs_removed_unless_keep)
EOF
    if [ "$found" = 1 ]; then
        say "Found on this computer:"
        printf '%s' "$found_lines"
        if [ -n "$cua_bin" ]; then
            say "  (the removal steps run the cua at $cua_bin)"
        fi
    fi
}

# ---------------------------------------------------------------- Spaces

# local_spaces: "ID STATUS" for each local Space.
local_spaces() {
    [ -n "$cua_bin" ] || return 0
    bounded 30 env CUA_TELEMETRY=0 DO_NOT_TRACK=1 "$cua_bin" sandbox ls --local --no-size 2>/dev/null |
        awk 'NR > 1 && $1 != "" && $1 != "No" && $1 != "ID" { print $1, $2 }' || true
}

# stop_spaces: lists the Spaces and stops the running ones (their disks stay).
stop_spaces() {
    [ -n "$spaces" ] || return 0
    say ""
    say "Spaces on this computer:"
    printf '%s\n' "$spaces" | sed 's/^/  /'
    while IFS= read -r row; do
        [ -n "$row" ] || continue
        id="${row%% *}"
        state="${row#* }"
        case "$state" in
        stopped | missing) continue ;;
        esac
        if [ "$dry_run" = 1 ]; then
            say "[dry-run] cua spaces stop $id"
        else
            info "stopping $id (its disk is kept)"
            bounded 90 env CUA_TELEMETRY=0 DO_NOT_TRACK=1 "$cua_bin" spaces stop "$id" >/dev/null 2>&1 ||
                warn "could not stop $id; it may still be running"
        fi
    done <<EOF
$spaces
EOF
}

delete_spaces_now() {
    [ "$delete_spaces" = 1 ] || return 0
    [ -n "$spaces" ] || return 0
    while IFS= read -r row; do
        [ -n "$row" ] || continue
        id="${row%% *}"
        if [ "$dry_run" = 1 ]; then
            say "[dry-run] cua sandbox rm $id --force"
        else
            info "deleting Space $id"
            bounded 120 env CUA_TELEMETRY=0 DO_NOT_TRACK=1 "$cua_bin" sandbox rm "$id" --force >/dev/null 2>&1 ||
                warn "could not delete $id"
        fi
    done <<EOF
$spaces
EOF
}

# ---------------------------------------------------------------- the removal steps

# The app's own executable only: the daemon it started is stopped, with the
# Spaces, by the steps after it.
app_patterns() {
    case "$os" in
    darwin) printf '%s\n' 'Cua Spaces\.app/Contents/MacOS/(CuaSpacesMac|Cua Spaces)( |$)' ;;
    linux)
        printf '%s\n' "^$sysroot/opt/Cua Spaces/cua-spaces( |\$)"
        printf '%s\n' '^/tmp/\.mount_[^/]*/cua-spaces( |$)'
        ;;
    esac
}

app_running() {
    while IFS= read -r pattern; do
        [ -n "$pattern" ] || continue
        if pgrep -f "$pattern" >/dev/null 2>&1; then return 0; fi
    done <<EOF
$(app_patterns)
EOF
    return 1
}

quit_app() {
    has pgrep || return 0
    app_running || return 0
    if [ "$dry_run" = 1 ]; then
        say "[dry-run] quit Cua Spaces"
        return 0
    fi
    info "quitting Cua Spaces"
    if [ "$os" = darwin ] && has osascript; then
        bounded 15 osascript -e 'tell application id "com.trycua.spaces.macos" to quit' >/dev/null 2>&1 || true
    fi
    waited=0
    while app_running && [ "$waited" -lt 10 ]; do
        sleep 1
        waited=$((waited + 1))
    done
    if app_running; then
        while IFS= read -r pattern; do
            [ -n "$pattern" ] || continue
            pkill -TERM -f "$pattern" >/dev/null 2>&1 || true
        done <<EOF
$(app_patterns)
EOF
        sleep 2
        while IFS= read -r pattern; do
            [ -n "$pattern" ] || continue
            pkill -KILL -f "$pattern" >/dev/null 2>&1 || true
        done <<EOF
$(app_patterns)
EOF
    fi
}

# The host service: `cua host remove` unregisters this computer from the relay
# and removes the service; the unit or plist is removed directly when cua is
# not there to do it.
remove_host_service() {
    while IFS= read -r h; do
        [ -n "$h" ] || continue
        if [ -f "$h/host/config.json" ] && [ -n "$cua_bin" ]; then
            if [ "$dry_run" = 1 ]; then
                say "[dry-run] CUA_HOME=$h cua host remove --force --uninstalling"
            else
                info "turning off hosting ($h)"
                bounded 150 env CUA_HOME="$h" CUA_TELEMETRY=0 DO_NOT_TRACK=1 "$cua_bin" host remove --force --uninstalling ||
                    warn "could not tell the relay this computer is gone; remove it from Machines in Cua Spaces"
            fi
        fi
    done <<EOF
$(cua_homes)
EOF
    case "$os" in
    darwin)
        old_agent=0
        [ -f "$HOME/Library/LaunchAgents/com.trycua.spacesd.host.plist" ] && old_agent=1
        for label in com.trycua.spacesd.host com.trycua.env-driver.host; do
            plist="$HOME/Library/LaunchAgents/$label.plist"
            [ -f "$plist" ] || continue
            if [ "$dry_run" = 1 ]; then
                say "[dry-run] launchctl bootout gui/$(id -u)/$label"
            else
                launchctl bootout "gui/$(id -u)/$label" >/dev/null 2>&1 || launchctl unload "$plist" >/dev/null 2>&1 || true
            fi
            rm_path "$plist"
        done
        # The Cua Spaces app's own host service (the app's executable as its
        # login item, since 0.10): `cua host remove` above unregisters it when
        # it can; this covers an app whose cua could not run. What it ran
        # (~/.cua/helper) goes too.
        while IFS= read -r app; do
            [ -n "$app" ] || continue
            [ -f "$app/Contents/Library/LaunchAgents/com.trycua.spaces.helper.plist" ] || continue
            [ -x "$app/Contents/MacOS/CuaSpacesMac" ] || continue
            if [ "$dry_run" = 1 ]; then
                say "[dry-run] $app/Contents/MacOS/CuaSpacesMac --service-helper unregister"
            else
                bounded 30 "$app/Contents/MacOS/CuaSpacesMac" --service-helper unregister >/dev/null 2>&1 || true
            fi
        done <<EOF
$(app_candidates)
EOF
        if [ "$dry_run" = 1 ]; then
            say "[dry-run] launchctl bootout gui/$(id -u)/com.trycua.spaces.helper"
        else
            launchctl bootout "gui/$(id -u)/com.trycua.spaces.helper" >/dev/null 2>&1 || true
        fi
        rm_path "$HOME/.cua/helper"
        # The privacy permissions macOS keeps for the app stay: an uninstall
        # never revokes them (see the driver uninstaller, 18 Oct 2026).
        if [ "$old_agent" = 1 ]; then
            info "System Settings may still list \"cua-spacesd\" under Privacy & Security (Screen Recording, Accessibility); select it and click the minus button to remove it"
        fi
        ;;
    linux)
        for unit in cua-spacesd-host cua-env-driver-host; do
            user_unit="$config_home/systemd/user/$unit.service"
            system_unit="$sysroot/etc/systemd/system/$unit.service"
            if [ -f "$user_unit" ]; then
                if [ "$dry_run" = 1 ]; then
                    say "[dry-run] systemctl --user disable --now $unit"
                elif has systemctl; then
                    bounded 30 systemctl --user disable --now "$unit" >/dev/null 2>&1 || true
                fi
                rm_path "$user_unit"
                [ "$dry_run" = 1 ] || { has systemctl && bounded 20 systemctl --user daemon-reload >/dev/null 2>&1 || true; }
            fi
            if [ -f "$system_unit" ] && [ "$(id -u)" = 0 ]; then
                if [ "$dry_run" = 1 ]; then
                    say "[dry-run] systemctl disable --now $unit"
                elif has systemctl; then
                    bounded 30 systemctl disable --now "$unit" >/dev/null 2>&1 || true
                fi
                rm_path "$system_unit"
                [ "$dry_run" = 1 ] || { has systemctl && bounded 20 systemctl daemon-reload >/dev/null 2>&1 || true; }
            elif [ -f "$system_unit" ]; then
                warn "the system service $system_unit needs root: sudo systemctl disable --now $unit && sudo rm $system_unit"
                failed=1
            fi
        done
        ;;
    esac
    # A host driver copy still running from the cua home.
    if [ -d "$cua_home/host/bin" ] && [ "$dry_run" = 0 ] && has pkill; then
        pkill -f "$cua_home/host/bin/" >/dev/null 2>&1 || true
    fi
}

# Whether this cua removes only Cua Spaces' entries from agents (`cua agents
# remove`: its own `cua` server and skills) and can preview that
# (`--dry-run`). Older builds removed every entry cua ever wrote, Cua Driver's
# `cua-driver` server and skill included, and had neither flag. The new flag
# in its help is the test.
cua_scopes_agents_remove() {
    [ -n "$cua_bin" ] || return 1
    bounded 20 env CUA_TELEMETRY=0 DO_NOT_TRACK=1 "$cua_bin" agents remove --help >"$tmp/agents-remove-help" 2>/dev/null || return 1
    grep -e '--include-cua-driver' "$tmp/agents-remove-help" >/dev/null 2>&1
}

# What a cua that is too old to leave Cua Driver's entries alone costs: its
# `agents remove` would strip them, so the step is skipped. That is a note, not a
# failure: the uninstall finishes normally (the scripts are published before the
# cua release that adds the flag). ~/.cua/agent-setup.json, the record
# `cua agents remove` works from, stays unless --delete-spaces.
OLD_CUA_AGENTS="this cua is older than the version that removes only Cua Spaces' entries from your agents: its 'cua agents remove' would also take Cua Driver's skill and MCP server out of Claude Code, Codex, Cursor and your other agents. So the agent step is skipped, and Cua Spaces' 'cua' MCP server and skills (cua-spaces, cua-volume, cua-sandboxes, gui-automation) stay in your agents. To remove them later, install the latest cua (curl -fsSL https://cua.ai/install.sh | sh -s -- --cli-only --yes) and run: cua agents remove"

# Removes the Cua Spaces skills and MCP entries (the `cua` server, `cua-local`
# for Local) from the agents. Cua Driver's `cua-driver` skill and server, and
# every other tool's entries, stay, unless --all asked for Cua Driver to go.
# With --dry-run it runs the same command with --dry-run, so the list is what
# the real run removes.
remove_agent_integrations() {
    [ -n "$cua_bin" ] || return 0
    set -- agents remove --agents all
    if [ "$all_tools" = 1 ]; then
        set -- "$@" --include-cua-driver
    fi
    if ! cua_scopes_agents_remove; then
        if [ "$dry_run" = 1 ]; then
            say "[dry-run] cua $*: skipped, $OLD_CUA_AGENTS"
        else
            info "$OLD_CUA_AGENTS"
            if [ "$delete_spaces" = 1 ]; then
                info "--delete-spaces also deletes the record cua keeps of them (agent-setup.json), so remove them by hand"
            fi
        fi
        return 0
    fi
    if [ "$dry_run" = 1 ]; then
        say "[dry-run] cua $* --dry-run:"
        planned=0
        bounded 60 env CUA_TELEMETRY=0 DO_NOT_TRACK=1 "$cua_bin" "$@" --dry-run >"$tmp/agents-plan" 2>&1 || planned=$?
        # Whatever it listed, even when one agent's config could not be read:
        # a real run removes the rows that did not fail.
        sed -e '/^Dry run:/d' -e 's/^/[dry-run] /' "$tmp/agents-plan"
        if [ "$planned" != 0 ]; then
            warn "'cua $* --dry-run' reported a problem (see above), so the list may be incomplete"
        fi
        return 0
    fi
    run_cua "removing the Cua Spaces skills and MCP entries from your agents" 60 "$@" --yes
}

stop_daemon() {
    [ -n "$cua_bin" ] || return 0
    while IFS= read -r h; do
        [ -n "$h" ] || continue
        if [ "$dry_run" = 1 ]; then
            say "[dry-run] CUA_HOME=$h cua daemon stop"
        else
            bounded 30 env CUA_HOME="$h" CUA_TELEMETRY=0 DO_NOT_TRACK=1 "$cua_bin" daemon stop >/dev/null 2>&1 || true
        fi
    done <<EOF
$(cua_homes)
EOF
}

# Cua Volume is a mount (~/Cua Volume on macOS, ~/cua-volume on Linux): the
# daemon unmounts it when it stops. Make sure, and never delete files while it
# is mounted.
volume_mounted=0
app_removed=0
unmount_volume() {
    case "$os" in
    darwin) mount_point="$HOME/Cua Volume" ;;
    linux) mount_point="$HOME/cua-volume" ;;
    esac
    is_mounted "$mount_point" || return 0
    if [ "$dry_run" = 1 ]; then
        say "[dry-run] unmount $mount_point"
        return 0
    fi
    info "unmounting $mount_point"
    case "$os" in
    darwin) bounded 30 umount "$mount_point" >/dev/null 2>&1 || bounded 30 diskutil unmount force "$mount_point" >/dev/null 2>&1 || true ;;
    linux) bounded 30 fusermount -u "$mount_point" >/dev/null 2>&1 || bounded 30 umount "$mount_point" >/dev/null 2>&1 || true ;;
    esac
    if is_mounted "$mount_point"; then
        warn "$mount_point is still mounted; leaving it and the Cua Volume files alone"
        volume_mounted=1
        failed=1
    else
        rmdir_quiet "$mount_point"
    fi
}

is_mounted() {
    has mount || return 1
    mount 2>/dev/null | grep -F " on $1 " >/dev/null 2>&1
}

# Sign out: revokes the session and removes the stored credentials.
sign_out() {
    if [ -n "$cua_bin" ]; then
        run_cua "signing out (cua auth logout)" 60 auth logout
        if [ -d "$cua_home/envs/staging" ]; then
            if [ "$dry_run" = 1 ]; then
                say "[dry-run] CUA_ENV=staging cua auth logout"
            else
                bounded 60 env CUA_ENV=staging CUA_TELEMETRY=0 DO_NOT_TRACK=1 "$cua_bin" auth logout >/dev/null 2>&1 || true
            fi
        fi
    fi
    while IFS= read -r h; do
        [ -n "$h" ] || continue
        for name in $SESSION_FILES; do
            rm_path "$h/$name"
        done
    done <<EOF
$(cua_homes)
EOF
}

# remove_app: the Cua Spaces app and what it put outside its own folder.
remove_app() {
    case "$os" in
    darwin)
        while IFS= read -r app; do
            [ -n "$app" ] || continue
            if is_cua_app "$app"; then
                rm_path "$app"
                if [ "$dry_run" = 0 ] && [ ! -e "$app" ]; then app_removed=1; fi
            else
                info "leaving $app (not the Cua Spaces app)"
            fi
        done <<EOF
$(app_candidates)
EOF
        if [ "$(id -u)" = 0 ] && has pkgutil && pkgutil --pkg-info com.trycua.spaces.app >/dev/null 2>&1; then
            run pkgutil --forget com.trycua.spaces.app >/dev/null 2>&1 || true
        fi
        ;;
    linux)
        if deb_installed; then
            if [ "$dry_run" = 1 ]; then
                say "[dry-run] apt-get remove -y cua-spaces"
            elif [ "$(id -u)" = 0 ]; then
                info "removing the cua-spaces package"
                apt-get remove -y cua-spaces </dev/null >/dev/null 2>&1 || {
                    warn "could not remove the cua-spaces package: apt-get remove cua-spaces"
                    failed=1
                }
            elif has sudo && sudo -n true 2>/dev/null; then
                info "removing the cua-spaces package"
                sudo -n apt-get remove -y cua-spaces </dev/null >/dev/null 2>&1 || {
                    warn "could not remove the cua-spaces package: sudo apt-get remove cua-spaces"
                    failed=1
                }
            else
                warn "the cua-spaces package needs root: sudo apt-get remove cua-spaces"
                failed=1
            fi
        fi
        while IFS= read -r app; do
            [ -n "$app" ] || continue
            rm_path "$app"
        done <<EOF
$(app_candidates)
EOF
        for entry in "${prefix:+$prefix/share/applications/cua-spaces.desktop}" "$data_home/applications/cua-spaces.desktop" "$config_home/autostart/cua-spaces.desktop"; do
            [ -n "$entry" ] || continue
            if [ -f "$entry" ] && grep -q 'cua-spaces\|Cua Spaces' "$entry" 2>/dev/null; then
                rm_path "$entry"
            fi
        done
        rm_path "$data_home/cua-spaces"
        ;;
    esac
    while IFS= read -r lib; do
        [ -n "$lib" ] || continue
        rm_path "$lib"
    done <<EOF
$(library_dirs_removed_always)
EOF
    if [ "$keep_data" = 0 ]; then
        if [ "$os" = darwin ] && has defaults && [ "$dry_run" = 0 ]; then
            bounded 10 defaults delete com.trycua.spaces.macos >/dev/null 2>&1 || true
        fi
        while IFS= read -r lib; do
            [ -n "$lib" ] || continue
            rm_path "$lib"
        done <<EOF
$(library_dirs_removed_unless_keep)
EOF
    fi
}

remove_cli() {
    while IFS= read -r cli; do
        [ -n "$cli" ] || continue
        [ -e "$cli" ] || [ -L "$cli" ] || continue
        if is_cua_cli "$cli"; then
            rm_path "$cli"
        else
            info "leaving $cli (not the Cua CLI)"
        fi
    done <<EOF
$(cli_candidates)
EOF
}

# The PATH line that install.sh and the app wrote under $MARKER. Kept when
# cua-driver or lume still live in the same folder.
shared_bin_dirs() {
    for dir in "$HOME/.local/bin" "${prefix:+$prefix/bin}" /usr/local/bin; do
        [ -n "$dir" ] || continue
        if [ "$all_tools" = 0 ] && { [ -e "$dir/cua-driver" ] || [ -e "$dir/lume" ]; }; then
            printf '%s\n' "$dir"
        fi
    done
}

clean_profiles() {
    [ "$no_modify_path" = 0 ] || return 0
    shared="$(shared_bin_dirs | tr '\n' ' ')"
    seen_profiles=" "
    for profile in "${ZDOTDIR:-$HOME}/.zshrc" "$HOME/.zshrc" "$HOME/.bash_profile" "$HOME/.bashrc" "$HOME/.profile" "$config_home/fish/config.fish"; do
        [ -f "$profile" ] || continue
        case "$seen_profiles" in
        *" $profile "*) continue ;;
        esac
        seen_profiles="$seen_profiles$profile "
        grep -qF "$MARKER" "$profile" 2>/dev/null || continue
        if [ "$dry_run" = 1 ]; then
            say "[dry-run] remove the PATH line the cua installer added to $profile"
            continue
        fi
        edited="$tmp/profile.edited"
        awk -v marker="$MARKER" -v shared="$shared" '
            function dir_of(line, d) {
                d = line
                if (sub(/^export PATH="/, "", d)) { sub(/:\$PATH"$/, "", d); return d }
                if (sub(/^fish_add_path "/, "", d)) { sub(/"$/, "", d); return d }
                return ""
            }
            function is_path_line(line) {
                return line ~ /^export PATH="[^"]*:\$PATH"$/ || line ~ /^fish_add_path "[^"]*"$/
            }
            { lines[NR] = $0 }
            END {
                n = split(shared, kept, " ")
                for (k = 1; k <= n; k++) keep[kept[k]] = 1
                i = 1
                while (i <= NR) {
                    if (lines[i] == marker && i < NR && is_path_line(lines[i + 1]) && !(dir_of(lines[i + 1]) in keep)) {
                        if (out_n > 0 && out[out_n] == "") out_n--
                        i += 2
                        continue
                    }
                    out[++out_n] = lines[i]
                    i++
                }
                for (j = 1; j <= out_n; j++) print out[j]
            }' "$profile" >"$edited"
        if cmp -s "$edited" "$profile"; then
            info "kept the PATH line in $profile (cua-driver or lume still lives in that folder)"
        else
            if cat "$edited" >"$profile" 2>/dev/null; then
                info "removed the PATH line from $profile (open a new terminal to pick it up)"
            else
                warn "could not edit $profile; remove the '$MARKER' line and the one after it by hand"
                failed=1
            fi
        fi
    done
}

# clean_state: the cua home. Settings and local state go unless --keep-data;
# Spaces and their data only with --delete-spaces.
clean_state() {
    [ -d "$cua_home" ] || return 0
    if [ "$delete_spaces" = 1 ]; then
        if [ "$volume_mounted" = 1 ]; then
            warn "not deleting $cua_home while Cua Volume is mounted"
            return 0
        fi
        delete_home
        return 0
    fi
    while IFS= read -r h; do
        [ -n "$h" ] || continue
        for name in $SESSION_FILES; do
            rm_path "$h/$name"
        done
        if [ "$keep_data" = 0 ]; then
            for name in $DISPOSABLE; do
                rm_path "$h/$name"
            done
        fi
        rmdir_quiet "$h/volume"
    done <<EOF
$(cua_homes)
EOF
    if [ "$keep_data" = 1 ]; then
        info "kept $cua_home"
    elif [ "$dry_run" = 1 ]; then
        say "[dry-run] keep the Spaces and their data in $cua_home (--delete-spaces removes them)"
    elif [ -d "$cua_home" ]; then
        for env_home in "$cua_home"/envs/*; do
            rmdir_quiet "$env_home"
        done
        rmdir_quiet "$cua_home/envs"
        rmdir_quiet "$cua_home"
        if [ -d "$cua_home" ]; then
            kept=""
            for entry in "$cua_home"/* "$cua_home"/.[!.]*; do
                [ -e "$entry" ] && kept="$kept${entry##*/} "
            done
            info "kept in $cua_home: $kept(Spaces and their data; --delete-spaces removes them)"
        fi
    fi
}

# delete_home: the whole cua home, for --delete-spaces. A CUA_HOME that Cua did
# not create (no Cua files inside) is left alone.
delete_home() {
    if [ -n "${CUA_HOME:-}" ] && [ "$home_recognised" = 0 ]; then
        warn "CUA_HOME=$cua_home has no Cua files in it; leaving it alone"
        return 0
    fi
    rm_path "$cua_home"
}

# A CUA_HOME that was set by hand is only deleted when Cua files are in it,
# checked before any step removes some of them.
home_recognised=1
recognise_home() {
    [ -n "${CUA_HOME:-}" ] || return 0
    home_recognised=0
    for name in host sandboxes vmm daemon.json config.toml credentials.json spaces.json volume keyvault.sock; do
        if [ -e "$cua_home/$name" ]; then home_recognised=1; fi
    done
}

# macOS Keychain: the session and device key live under service run.cua.ai
# (staging.run.cua.ai for staging); the Keyvault master key under "Cua Keyvault".
kc_delete_all() { # service
    kc_has "$1" || return 0
    if [ "$dry_run" = 1 ]; then
        say "[dry-run] remove the Keychain items of service $1"
        return 0
    fi
    n=0
    while [ "$n" -lt 20 ] && bounded 20 security delete-generic-password -s "$1" >/dev/null 2>&1; do
        n=$((n + 1))
    done
    if kc_has "$1"; then
        warn "could not remove every Keychain item of '$1'; delete them in Keychain Access (search for $1)"
        failed=1
    else
        info "removed $n Keychain item(s) of '$1'"
    fi
}

kc_delete_account() { # service account
    bounded 20 security find-generic-password -s "$1" -a "$2" >/dev/null 2>&1 || return 0
    if [ "$dry_run" = 1 ]; then
        say "[dry-run] remove the Keychain item $1 / $2"
        return 0
    fi
    if bounded 20 security delete-generic-password -s "$1" -a "$2" >/dev/null 2>&1; then
        info "removed the Keychain item $1 / $2"
    else
        warn "could not remove the Keychain item $1 / $2; delete it in Keychain Access"
        failed=1
    fi
}

remove_secrets() {
    case "$os" in
    darwin)
        has security || return 0
        for service in run.cua.ai staging.run.cua.ai; do
            if [ "$keep_data" = 1 ]; then
                kc_delete_account "$service" cua-cli
            else
                kc_delete_all "$service"
            fi
        done
        if [ "$delete_spaces" = 1 ]; then
            kc_delete_all "Cua Keyvault"
            kc_delete_all "Cua Keyvault Generation"
        fi
        ;;
    linux)
        # Only used when the Secret Service holds Cua items (forced keychain store, Keyvault).
        has secret-tool || return 0
        if [ "$dry_run" = 1 ]; then
            say "[dry-run] secret-tool clear service run.cua.ai"
            return 0
        fi
        if [ "$keep_data" = 0 ]; then
            bounded 20 secret-tool clear service run.cua.ai >/dev/null 2>&1 || true
            bounded 20 secret-tool clear service staging.run.cua.ai >/dev/null 2>&1 || true
        fi
        if [ "$delete_spaces" = 1 ]; then
            bounded 20 secret-tool clear service "Cua Keyvault" >/dev/null 2>&1 || true
            bounded 20 secret-tool clear service "Cua Keyvault Generation" >/dev/null 2>&1 || true
        fi
        ;;
    esac
}

# ---------------------------------------------------------------- other Cua tools

check_url() {
    case "$1" in
    *[[:space:]]* | http://*@*) die "refusing a malformed URL: $1" ;;
    https://*) ;;
    http://127.0.0.1[:/]* | http://localhost[:/]* | http://\[::1\][:/]*) ;;
    *) die "refusing a non-HTTPS download: $1" ;;
    esac
}

fetch() { # URL DEST
    check_url "$1"
    if has curl; then
        case "$1" in
        https://*) proto='=https' ;;
        *) proto='=https,http' ;;
        esac
        curl --proto "$proto" --proto-redir "$proto" --tlsv1.2 -fsSL --retry 3 -o "$2" "$1"
    elif has wget; then
        wget -q -O "$2" "$1"
    else
        return 1
    fi
}

# run_other_uninstaller NAME URL ARGS...: Cua Driver's and Lume's own
# uninstallers (bash scripts), the same ones cua.ai serves.
run_other_uninstaller() {
    name="$1"
    url="$2"
    shift 2
    if [ "$dry_run" = 1 ]; then
        say "[dry-run] download $url"
        say "[dry-run] bash $name-uninstall.sh${*:+ $*}"
        return 0
    fi
    has bash || {
        warn "$name: its uninstaller needs bash; run it by hand: /bin/bash -c \"\$(curl -fsSL $url)\""
        failed=1
        return 0
    }
    script="$tmp/$name-uninstall.sh"
    if ! fetch "$url" "$script"; then
        warn "$name: could not download $url"
        failed=1
        return 0
    fi
    info "running the $name uninstaller"
    bash "$script" "$@" </dev/null || {
        warn "the $name uninstaller did not finish"
        failed=1
    }
}

remove_other_tools() {
    [ "$all_tools" = 1 ] || return 0
    run_other_uninstaller cua-driver "$DRIVER_URL"
    if [ "$os" = darwin ]; then
        if [ "$delete_spaces" = 1 ]; then
            run_other_uninstaller lume "$LUME_URL" --force --purge
        else
            run_other_uninstaller lume "$LUME_URL" --force
        fi
    fi
}

# ---------------------------------------------------------------- main

confirm_plan() {
    [ "$dry_run" = 0 ] || return 0
    say ""
    say "This removes Cua Spaces, the cua CLI, the host service, Cua Spaces' skills and MCP"
    say "entry in your agents, launch at login and your sign-in."
    if [ "$all_tools" = 0 ]; then
        say "Cua Driver's skill and MCP server in your agents, and other tools' entries, stay."
    fi
    if [ "$keep_data" = 1 ]; then
        say "It keeps $cua_home and the Cua Volume cache (--keep-data)."
    elif [ "$delete_spaces" = 1 ]; then
        say "It also deletes Cua's settings and caches."
    else
        say "It also deletes Cua's settings and caches, but keeps your Spaces and their data."
    fi
    if [ "$all_tools" = 1 ]; then
        say "It also runs the Cua Driver and Lume uninstallers, and removes Cua Driver's skill and"
        say "MCP server from your agents (--all)."
    fi
    if [ "$assume_yes" = 0 ] && ! tty_ok; then
        die "no terminal to ask on; rerun with --yes (or --dry-run to see what would be removed)"
    fi
    confirm "Continue?" || {
        say "Cancelled; nothing was removed."
        exit 0
    }
    if [ "$delete_spaces" = 1 ]; then
        say ""
        say "--delete-spaces DELETES the Spaces on this computer and their data:"
        if [ -n "${spaces:-}" ]; then
            printf '%s\n' "$spaces" | sed 's/^/  /'
        fi
        say "  $cua_home: Space disks, the local Cua Volume files, the Keyvault. This cannot be undone."
        if [ "$assume_yes" = 0 ]; then
            printf 'Type "delete" to delete them, or anything else to cancel: ' >/dev/tty
            answer=""
            read -r answer </dev/tty || answer=""
            [ "$answer" = delete ] || {
                say "Cancelled; nothing was removed."
                exit 0
            }
        fi
    fi
}

main() {
    parse_args "$@"
    detect_platform
    tmp="$(mktemp -d "${TMPDIR:-/tmp}/cua-uninstall.XXXXXX" 2>/dev/null || mktemp -d -t cua-uninstall)"
    find_cua
    survey
    if [ "$found" = 0 ] && [ "$all_tools" = 0 ]; then
        info "Cua does not look installed here; nothing to remove"
        exit 0
    fi
    if [ "$dry_run" = 1 ]; then
        info "dry run: nothing will be changed"
    fi
    recognise_home
    spaces="$(local_spaces)"
    confirm_plan
    quit_app
    stop_spaces
    delete_spaces_now
    remove_host_service
    remove_agent_integrations
    stop_daemon
    unmount_volume
    sign_out
    remove_app
    remove_cli
    remove_other_tools
    clean_profiles
    clean_state
    remove_secrets
    if [ "$dry_run" = 1 ]; then
        info "dry run finished; nothing was changed"
        exit 0
    fi
    if has cua && [ "$all_tools" = 0 ]; then
        info "another cua is still on your PATH at $(command -v cua); it was not installed by Cua's installer (pip, npm or Homebrew?), so remove it with the tool that installed it"
    fi
    if [ "$all_tools" = 0 ] && { has cua-driver || [ -e "$HOME/.local/bin/cua-driver" ]; }; then
        info "Cua Driver is separate and was kept; run this with --all to remove it too"
    fi
    if [ "$app_removed" = 1 ]; then
        info "the app registered itself as a login item, which a script cannot unregister; if Cua Spaces still shows in System Settings, Login Items, switch it off there"
    fi
    if [ "$failed" = 1 ]; then
        info "finished, but some steps need your attention (see the warnings above)"
        exit 1
    fi
    info "done. To install again: curl -fsSL https://cua.ai/install.sh | sh"
}

main "$@"
