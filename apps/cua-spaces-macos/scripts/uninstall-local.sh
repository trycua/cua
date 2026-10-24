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
#   - the Local helper login item (unregistered) and the Local host runner job
#   - ~/Applications/Cua Spaces Local.app
#   - ~/.local/bin/cua-local
#   - ~/.cua-local (the Local sign-in, host and Spaces state)
#     and ~/Library/Application Support/com.trycua.spaces.local
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
APP_DEST="$PREFIX/Applications/$LOCAL_APP_DIR_NAME"
CLI_LINK="$PREFIX/.local/bin/$LOCAL_CLI_NAME"
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
case "$APP_DEST" in
    /Applications/*) die "refusing to remove anything under /Applications (that is where Cua Spaces lives): use the --prefix you installed with." ;;
esac

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
echo "  Keychain items:     ${LOCAL_KEYCHAIN_SERVICES[*]}"
if [ "$YES" != 1 ]; then
    confirm "Remove them?" || { log "cancelled, nothing was removed"; exit 0; }
fi

REMOVE_LUME=0
if [ "$WITH_LUME_LOCAL" = 1 ]; then
    REMOVE_LUME=1
elif [ "$YES" != 1 ] && lume_local_present; then
    confirm "Also remove $LOCAL_LUME_BIN_NAME and its VMs in ~/$LOCAL_LUME_DIR_NAME?" && REMOVE_LUME=1
fi

# --- The helper login item and the host runner ------------------------------------

# Only this bundle's own code may unregister its login item, so ask the app
# (the same call the host setup makes). Without the app, stop the job.
if [ -x "$APP_DEST/Contents/MacOS/CuaSpacesMac" ]; then
    log "unregistering the helper login item"
    CUA_CHANNEL=local "$APP_DEST/Contents/MacOS/CuaSpacesMac" --service-helper unregister >/dev/null 2>&1 \
        || warn "the app could not unregister its helper; remove $LOCAL_APP_DIR_NAME from System Settings > Login Items if it stays."
fi
quit_local_app "$APP_DEST"
launchctl bootout "$GUI_DOMAIN/$LOCAL_HELPER_LABEL" >/dev/null 2>&1 || true
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
