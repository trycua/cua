#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Removes Cua Spaces Local (CUA-1776), the build scripts/install-local.sh
# installs, and only that. The released Cua Spaces, ~/.cua, ~/.local/bin/cua,
# ~/.lume and their launchd jobs and Keychain items have other names and are
# never reached.
#
#   scripts/uninstall-local.sh [--yes] [--with-lume-local] [--prefix DIR]
#
#   --yes, -y            do not ask first
#   --with-lume-local    also remove lume-local (without it, it asks, and with
#                        --yes it stays); its VMs in ~/.lume-local go with it
#   --prefix DIR         where install-local.sh put the app and `cua-local`
#                        (default $HOME); the state is always removed from $HOME
#
# It removes, after asking:
#   - this Mac's entry in the relay, in each environment Local is signed in to
#     (best effort: the machine would otherwise stay listed as offline), before
#     the sign-in goes
#   - Local's MCP server (`cua-local`) and skills (`*-local`) in Claude Code and
#     the other coding agents (`cua-local agents remove`, best effort), before
#     the CLI and ~/.cua-local, which records what Local wrote, go; the released
#     app's entries and `cua-driver` stay
#   - the Local helper login item (unregistered) and the Local host runner job
#   - ~/Applications/Cua Spaces Local.app
#   - ~/.local/bin/cua-local
#   - ~/.cua-local (the Local sign-in, host and Spaces state)
#     and ~/Library/Application Support/com.trycua.spaces.local
#   - what macOS keeps for the Local bundle id under ~/Library: its preferences,
#     caches, web view data, cookies and saved window state
#   - the same for the old Local bundle id (com.trycua.spaces.local, before the
#     rename to com.trycua.spaces.macos.local): an app with that id at the Local
#     path and its ~/Library files
#   - the Local Keychain items (ai.cua.session.local, ai.cua.session.local.staging)
#   - optionally lume-local (libs/lume/scripts/uninstall-local.sh)
# It keeps the Cua Volume Local folder and the permissions macOS granted the
# app. It refuses, changing nothing, when the app at the Local path is not a
# Cua Spaces Local bundle.
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/../../.." && pwd)"

# shellcheck source=_local-common.sh
# shellcheck source-path=SCRIPTDIR
. "$SCRIPT_DIR/_local-common.sh"

die() { err "$*"; exit 1; }

YES=0
PREFIX=""
WITH_LUME_LOCAL=0

usage() { sed -n '/^# Removes Cua Spaces Local/,/^# Cua Spaces Local bundle\./p' "$0" | sed 's/^# \{0,1\}//'; }

while [ "$#" -gt 0 ]; do
    case "$1" in
        --yes|-y) YES=1 ;;
        --with-lume-local) WITH_LUME_LOCAL=1 ;;
        --prefix)
            [ "$#" -ge 2 ] || die "--prefix needs a directory"
            PREFIX="$2"
            shift
            ;;
        --prefix=*) PREFIX="${1#*=}" ;;
        --help|-h) usage; exit 0 ;;
        *) err "unknown option: $1 (see --help)"; exit 2 ;;
    esac
    shift
done

[ "$(uname -s)" = Darwin ] || die "Cua Spaces Local is a macOS app: run this on a Mac."
[ -n "${HOME:-}" ] && [ "$HOME" != / ] || die "HOME is not set to a user home."

if [ -z "$PREFIX" ]; then
    PREFIX="$HOME"
fi
case "$PREFIX" in
    /*) ;;
    *) die "--prefix must be an absolute path (got: $PREFIX)" ;;
esac
PREFIX="${PREFIX%/}"
# Resolved like install-local.sh does (`..`, symlinks), so what is removed is
# what was installed and never anything of the released Cua Spaces.
APP_PARENT="$(canonicalize_path "$PREFIX/Applications")" || die "cannot resolve $PREFIX/Applications (a symlink loop?)"
BIN_DIR="$(canonicalize_path "$PREFIX/.local/bin")" || die "cannot resolve $PREFIX/.local/bin (a symlink loop?)"
APP_DEST="$APP_PARENT/$LOCAL_APP_DIR_NAME"
CLI_LINK="$BIN_DIR/$LOCAL_CLI_NAME"
LOCAL_HOME="$HOME/$LOCAL_HOME_DIR_NAME"
LOCAL_APP_SUPPORT="$HOME/Library/Application Support/$LOCAL_APP_SUPPORT_DIR_NAME"
HOST_JOB_PLIST="$HOME/Library/LaunchAgents/$LOCAL_HOST_LABEL.plist"
LUME_UNINSTALL="$REPO_ROOT/libs/lume/scripts/uninstall-local.sh"
GUI_DOMAIN="gui/$(id -u)"

# Every path this script removes has a Local name; these are the checks that
# keep a bad prefix or HOME from aiming it elsewhere.
[ "$(basename "$APP_DEST")" = "$LOCAL_APP_DIR_NAME" ] || die "internal error: app path $APP_DEST"
[ "$(basename "$CLI_LINK")" = "$LOCAL_CLI_NAME" ] || die "internal error: CLI path $CLI_LINK"
[ "$(basename "$LOCAL_HOME")" = "$LOCAL_HOME_DIR_NAME" ] || die "internal error: home $LOCAL_HOME"
reject_stable_overlap "app" "$APP_DEST" || exit 1
reject_stable_overlap "$LOCAL_CLI_NAME link" "$CLI_LINK" || exit 1

# Refuse before changing anything.
require_local_bundle_or_absent "$APP_DEST" || exit 1

# Is lume-local present, and may its uninstaller be called?
lume_local_present() {
    [ -e "$PREFIX/.local/bin/$LOCAL_LUME_BIN_NAME" ] || [ -L "$PREFIX/.local/bin/$LOCAL_LUME_BIN_NAME" ] \
        || [ -d "$HOME/$LOCAL_LUME_DIR_NAME" ]
}

confirm() {
    local reply
    printf '%s [y/N] ' "$1"
    read -r reply || reply=""
    case "$reply" in y|Y|yes|YES) return 0 ;; *) return 1 ;; esac
}

echo "This removes Cua Spaces Local. The released Cua Spaces, ~/.cua, ~/.local/bin/cua and ~/.lume are not touched."
echo "  helper login item:  $LOCAL_HELPER_LABEL"
echo "  app:                $APP_DEST"
echo "  CLI:                $CLI_LINK"
echo "  state:              $LOCAL_HOME and $LOCAL_APP_SUPPORT"
echo "  macOS leftovers:    the $LOCAL_BUNDLE_ID preferences, caches, web view data and window state in ~/Library"
echo "                      (and those of the old id $LOCAL_LEGACY_BUNDLE_ID, if any)"
echo "  Keychain items:     ${LOCAL_KEYCHAIN_SERVICES[*]}"
echo "  relay:              this Mac's entry, if Local is signed in"
echo "  coding agents:      Local's MCP server ($LOCAL_CLI_NAME) and its *-local skills; the released app's entries stay"
if [ "$YES" != 1 ]; then
    confirm "Remove them?" || { log "cancelled, nothing was removed"; exit 0; }
fi

REMOVE_LUME=0
if [ "$WITH_LUME_LOCAL" = 1 ]; then
    REMOVE_LUME=1
elif [ "$YES" != 1 ] && lume_local_present; then
    confirm "Also remove $LOCAL_LUME_BIN_NAME and its VMs in ~/$LOCAL_LUME_DIR_NAME?" && REMOVE_LUME=1
fi

# --- This Mac's entry in the relay, and Local's entries in your coding agents --------

# The bundled Local CLI for one environment (staging, or prod for the opt-in),
# as Local and nothing else: every CUA_ variable of the caller's shell is
# dropped, so a stray CUA_HOME or CUA_API_KEY for the released Cua Spaces can
# neither aim it at that install nor hand its key to this one, and CUA_ENV is
# the one asked for. It gives up after 30 seconds (CUA_UNINSTALL_CLI_SECS
# changes that): the CLI runs in a process group of its own, and the whole group
# is stopped, so a program it started (`claude mcp remove`) cannot go on editing
# an agent's configuration after the uninstall has finished.
run_local_cli() {
    local cli="$1" env="$2" secs="${CUA_UNINSTALL_CLI_SECS:-30}" var
    shift 2
    (
        for var in $(compgen -e); do
            case "$var" in CUA_*) unset "$var" ;; esac
        done
        export CUA_CHANNEL=local CUA_ENV="$env" CUA_TELEMETRY=0
        exec /usr/bin/perl -e '
            my $secs = shift @ARGV;
            setpgrp(0, 0);
            my $pid = fork();
            exit 127 unless defined $pid;
            if (!$pid) { exec @ARGV or exit 127 }
            $SIG{ALRM} = sub {
                $SIG{TERM} = "IGNORE";
                kill "TERM", -$$;
                my $until = time + 2;
                while (time < $until && waitpid($pid, 1) == 0) { select(undef, undef, undef, 0.1) }
                kill "KILL", -$$;
                exit 124;
            };
            alarm $secs;
            waitpid($pid, 0);
            my $st = $?;
            exit($st & 127 ? 128 + ($st & 127) : $st >> 8);
        ' -- "$secs" "$cli" "$@"
    )
}

# Succeeds when every link in `dir` (down to `maxdepth` levels) resolves to a
# place of its own and not to anything of the released Cua Spaces, and so does
# every link inside the folders those links lead to (`levels` folders deep).
# `find -H` looks inside `dir` when `dir` itself is a link.
links_are_isolated() {
    local dir="$1" levels="$2" maxdepth="$3" entry resolved
    [ "$levels" -gt 0 ] || return 0
    while IFS= read -r entry; do
        resolved="$(canonicalize_path "$entry")" || return 1
        reject_stable_overlap "Local state" "$resolved" >/dev/null 2>&1 || return 1
        if [ -d "$resolved" ]; then
            links_are_isolated "$resolved" $((levels - 1)) 4 || return 1
        fi
    done < <(find -H "$dir" -maxdepth "$maxdepth" -type l 2>/dev/null)
}

# Whether a Local home may be handed to the Local CLI: it, the folders the CLI
# reads and removes in it, and every link in them, however indirect, resolve to
# places of their own and not to anything of the released Cua Spaces (a link such
# as ~/.cua-local -> ~/.cua, or ~/.cua-local/host/config.json ->
# ~/.cua/host/config.json, or a host folder elsewhere that holds such a link,
# would make the CLI unregister and delete Stable's host or its Spaces).
local_home_is_isolated() {
    local home="$1" sub resolved
    for sub in "" /host /helper /agent-setup.json; do
        resolved="$(canonicalize_path "$home$sub")" || return 1
        reject_stable_overlap "Local state" "$resolved" >/dev/null 2>&1 || return 1
    done
    links_are_isolated "$home" 3 1 || return 1
    links_are_isolated "$home/host" 3 4 || return 1
    links_are_isolated "$home/helper" 3 4 || return 1
}

# Takes this Mac off the relay, while the sign-in and the host's machine token
# still exist: they are what proves it may. Best effort. Not signed in (or no
# app left to ask): there is nothing to unregister, so it says nothing.
take_off_relay() {
    local cli="$1" env="$2"
    run_local_cli "$cli" "$env" auth status >/dev/null 2>&1 || return 0
    log "taking this Mac off the $env relay"
    if ! run_local_cli "$cli" "$env" host remove --force --uninstalling >/dev/null 2>&1; then
        warn "could not take this Mac off the $env relay (offline?): it may stay listed, offline, in your computers until you remove it there."
    fi
}

# Takes Local's MCP server (`cua-local`) and skills (`*-local`) out of Claude
# Code and the other coding agents, while the home that records what Local
# wrote is still there: without it they would point at a deleted binary.
# `agents remove` undoes only what this home's record says Local wrote, so the
# released Cua Spaces' `cua` entry and skills, and `cua-driver`, stay. Best
# effort, with the same limit as the relay call; it needs no sign-in.
remove_agent_entries() {
    local cli="$1" env="$2"
    log "removing Local's MCP server and skills from your coding agents"
    if ! run_local_cli "$cli" "$env" agents remove --yes >/dev/null 2>&1; then
        warn "could not remove Local's entries from your coding agents ($env): the $LOCAL_CLI_NAME MCP server and the *-local skills may still be listed there; remove them by hand."
    fi
}

# For each environment Local has state for (staging, and prod when Local was
# opted in to it): off the relay, then out of the coding agents. Nothing is
# asked of the Local CLI when it is missing (a half-removed install) or when the
# home leads into the released Cua Spaces' folders.
unregister_local() {
    local cli="$APP_DEST/Contents/MacOS/cua" env home
    [ -f "$cli" ] && [ -x "$cli" ] && [ ! -L "$cli" ] || return 0
    for env in staging prod; do
        case "$env" in
            staging) home="$LOCAL_HOME" ;;
            *) home="$LOCAL_HOME/envs/prod" ;;
        esac
        [ -d "$home" ] || continue
        if ! local_home_is_isolated "$home"; then
            warn "not taking this Mac off the $env relay or removing its coding agent entries: $home leads into the released Cua Spaces' folders."
            continue
        fi
        take_off_relay "$cli" "$env"
        remove_agent_entries "$cli" "$env"
    done
}
unregister_local

# --- The helper login item and the host runner ------------------------------------

# Only this bundle's own code may unregister its login item, so ask the app
# (the same call the host setup makes). Without the app, stop the job. An app
# with the old Local id at this path is handled the same way.
if is_legacy_local_bundle "$APP_DEST"; then
    log "found the old ${LOCAL_APP_DIR_NAME%.app} (bundle id $LOCAL_LEGACY_BUNDLE_ID); removing it too"
fi
if [ -x "$APP_DEST/Contents/MacOS/CuaSpacesMac" ]; then
    log "unregistering the helper login item"
fi
retire_local_app "$APP_DEST"
# The plain runner (`cua host setup` without the app) has its own Local label.
launchctl bootout "$GUI_DOMAIN/$LOCAL_HOST_LABEL" >/dev/null 2>&1 || true
if [ -f "$HOST_JOB_PLIST" ]; then
    rm -f "$HOST_JOB_PLIST"
    log "removed $HOST_JOB_PLIST"
fi

# --- The app and the CLI ---------------------------------------------------------------

if [ -d "$APP_DEST" ]; then
    if [ -x "$LSREGISTER" ]; then
        "$LSREGISTER" -u "$APP_DEST" >/dev/null 2>&1 || true
    fi
    rm -rf "$APP_DEST"
    log "removed $APP_DEST"
fi
# A backup left by an interrupted install of the Local app.
for backup in "$APP_DEST".install-backup.*; do
    [ -d "$backup" ] || continue
    rm -rf "$backup"
    log "removed $backup"
done

# `cua-local` is the Local CLI by name: the install links it into the bundle,
# the app's first run may copy it. Either way the name is Local's.
if [ -d "$CLI_LINK" ] && [ ! -L "$CLI_LINK" ]; then
    warn "$CLI_LINK is a directory; leaving it"
elif [ -e "$CLI_LINK" ] || [ -L "$CLI_LINK" ]; then
    rm -f "$CLI_LINK"
    log "removed $CLI_LINK"
fi

# --- State -------------------------------------------------------------------------------

if [ -d "$LOCAL_HOME" ] || [ -L "$LOCAL_HOME" ]; then
    rm -rf "$LOCAL_HOME"
    log "removed $LOCAL_HOME"
fi
if [ -d "$LOCAL_APP_SUPPORT" ]; then
    rm -rf "$LOCAL_APP_SUPPORT"
    log "removed $LOCAL_APP_SUPPORT"
fi

# What macOS keeps for the Local bundle id (every name carries it; Stable's
# folders have its own id and are never listed). The app is stopped above, so
# it cannot write them back.
while IFS= read -r leaf; do
    case "$leaf" in
        *"$LOCAL_BUNDLE_ID"*) ;;
        *) die "internal error: $leaf is not a Local path" ;;
    esac
    leftover="$HOME/Library/$leaf"
    if [ -e "$leftover" ] || [ -L "$leftover" ]; then
        rm -rf "$leftover"
        log "removed $leftover"
    fi
done < <(library_leaves "$LOCAL_BUNDLE_ID")

# The old Local bundle id's leftovers, from before the rename: the same kinds
# of files under the old id (none is a name of the current id or Stable's).
while IFS= read -r leftover; do
    [ -n "$leftover" ] || continue
    log "removed $leftover"
done < <(remove_legacy_local_data)

# The Keychain tool deletes one item per call and has no wildcard: repeat per
# service (one item per account) until none is left. Local services only.
if command -v security >/dev/null 2>&1; then
    for service in "${LOCAL_KEYCHAIN_SERVICES[@]}"; do
        removed=0
        while [ "$removed" -lt 50 ] && security delete-generic-password -s "$service" >/dev/null 2>&1; do
            removed=$((removed + 1))
        done
        [ "$removed" = 0 ] || log "removed $removed Keychain item(s) of $service"
    done
fi

# --- lume-local ----------------------------------------------------------------------------

if [ "$REMOVE_LUME" = 1 ]; then
    if [ ! -f "$LUME_UNINSTALL" ]; then
        warn "lume-local was not removed: this checkout has no libs/lume/scripts/uninstall-local.sh."
    elif ! grep -q "$LOCAL_LUME_BIN_NAME" "$LUME_UNINSTALL"; then
        warn "$LOCAL_LUME_BIN_NAME was not removed: $LUME_UNINSTALL does not know it."
    else
        log "removing lume-local"
        lume_args=()
        # Not asking twice: it was confirmed above.
        if grep -q -- '--yes' "$LUME_UNINSTALL"; then
            lume_args+=(--yes)
        elif grep -q -- '--force' "$LUME_UNINSTALL"; then
            lume_args+=(--force)
        fi
        /bin/bash "$LUME_UNINSTALL" ${lume_args[@]+"${lume_args[@]}"}
    fi
fi

echo ""
ok "Cua Spaces Local was removed; the released Cua Spaces was left untouched."
echo "Kept: ~/$LOCAL_VOLUME_NAME (your files) and the permissions macOS granted $LOCAL_BUNDLE_ID."
