#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Names and checks shared by install-local.sh and uninstall-local.sh (Cua
# Spaces Local, CUA-1776). Sourced; no side effects besides the definitions.
#
# THE NAMES BELOW MIRROR `CuaChannel::Local` in
# libs/cua/crates/cua-home/src/channel.rs, which owns them. A shell script
# cannot call that module, so they are written once, here, and
# scripts/tests/install-local-test.sh fails when one stops matching it.
# Nothing else in these scripts spells a Local or a Stable name.

# shellcheck disable=SC2034 # each script uses some of these names

# --- Local (what these scripts install and remove) ---------------------------
LOCAL_BUNDLE_ID="com.trycua.spaces.local"          # CuaChannel::bundle_id
LOCAL_APP_DIR_NAME="Cua Spaces Local.app"          # CuaChannel::app_path
LOCAL_CLI_NAME="cua-local"                         # CuaChannel::cli_name
LOCAL_URL_SCHEME="cua-local"                       # CuaChannel::url_scheme
LOCAL_HOME_DIR_NAME=".cua-local"                   # CuaChannel::home_dir
LOCAL_APP_SUPPORT_DIR_NAME="com.trycua.spaces.local" # CuaChannel::app_support_dir_name
LOCAL_HELPER_LABEL="com.trycua.spaces.local.helper"  # CuaChannel::helper_label
LOCAL_HOST_LABEL="com.trycua.spacesd.host.local"     # CuaChannel::host_label
LOCAL_VOLUME_NAME="Cua Volume Local"               # CuaChannel::volume_name
LOCAL_LUME_BIN_NAME="lume-local"                   # CuaChannel::lume_bin_name
LOCAL_LUME_DIR_NAME=".lume-local"                  # CuaChannel::lume_dir
# CuaChannel::keychain_service(Prod) and (Staging). The sign-in and the device
# key live under these services, one item per account.
LOCAL_KEYCHAIN_SERVICES=("ai.cua.session.local" "ai.cua.session.local.staging")

# --- Stable (what these scripts must never create, replace or remove) --------
# Only used to refuse a bundle that carries a Stable identity.
STABLE_BUNDLE_ID="com.trycua.spaces.macos"          # also its app support folder name
STABLE_URL_SCHEME="cua"
STABLE_HELPER_LABEL="com.trycua.spaces.helper"
STABLE_APP_PARENT="/Applications"                   # CuaChannel::app_path
STABLE_APP_DIR_NAME="Cua Spaces.app"
STABLE_HOME_DIR_NAME=".cua"                         # CuaChannel::home_dir
STABLE_CLI_NAME="cua"                               # CuaChannel::cli_name
STABLE_LUME_DIR_NAME=".lume"                        # CuaChannel::lume_dir
STABLE_VOLUME_NAME="Cua Volume"                     # CuaChannel::volume_name

PLISTBUDDY=/usr/libexec/PlistBuddy
LSREGISTER="${CUA_LSREGISTER:-/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister}"

BOLD=$(tput bold 2>/dev/null || true)
NORMAL=$(tput sgr0 2>/dev/null || true)
RED=$(tput setaf 1 2>/dev/null || true)
GREEN=$(tput setaf 2 2>/dev/null || true)
BLUE=$(tput setaf 4 2>/dev/null || true)
YELLOW=$(tput setaf 3 2>/dev/null || true)

log() { printf '%s==>%s %s\n' "$BLUE" "$NORMAL" "$*"; }
ok() { printf '%s%s%s\n' "$GREEN" "$*" "$NORMAL"; }
warn() { printf '%swarning: %s%s\n' "$YELLOW" "$*" "$NORMAL" >&2; }
err() { printf '%serror: %s%s\n' "$RED" "$*" "$NORMAL" >&2; }

# A value of a bundle's Info.plist (a key path like CFBundleIdentifier); empty
# and failing when it is not there.
plist_get() { "$PLISTBUDDY" -c "Print :$2" "$1" 2>/dev/null; }

# The bundle identifier of an app bundle, empty when it has none.
bundle_id_of() { plist_get "$1/Contents/Info.plist" CFBundleIdentifier || true; }

# Every URL scheme the bundle registers, one per line.
plist_url_schemes() {
    local plist="$1" i=0 j value
    while plist_get "$plist" "CFBundleURLTypes:$i" >/dev/null; do
        j=0
        while value="$(plist_get "$plist" "CFBundleURLTypes:$i:CFBundleURLSchemes:$j")"; do
            printf '%s\n' "$value"
            j=$((j + 1))
        done
        i=$((i + 1))
    done
}

# Succeeds when `path` is absent or a Cua Spaces Local bundle, the only thing
# these scripts may replace or remove. Anything else, a Stable app included, is
# refused with the reason.
require_local_bundle_or_absent() {
    local app="$1" id
    if [ ! -e "$app" ] && [ ! -L "$app" ]; then
        return 0
    fi
    if [ -L "$app" ]; then
        err "refusing to touch $app: it is a symlink, not a Cua Spaces Local bundle"
        return 1
    fi
    if [ ! -f "$app/Contents/Info.plist" ]; then
        err "refusing to touch $app: it is not an app bundle (no Contents/Info.plist)"
        return 1
    fi
    id="$(bundle_id_of "$app")"
    if [ "$id" != "$LOCAL_BUNDLE_ID" ]; then
        err "refusing to touch $app: its bundle id is '${id:-none}', not $LOCAL_BUNDLE_ID"
        return 1
    fi
    return 0
}

# Stops the running Local app, its helper included, and nothing else: only
# processes started from inside this bundle are matched.
quit_local_app() {
    local app="$1" pattern pids waited=0
    command -v pgrep >/dev/null 2>&1 || return 0
    pattern="^$(printf '%s' "$app" | sed 's/[][\.*^$+?(){}|]/\\&/g')/Contents/"
    pids="$(pgrep -f "$pattern" 2>/dev/null || true)"
    [ -n "$pids" ] || return 0
    log "stopping the running $LOCAL_APP_DIR_NAME"
    # shellcheck disable=SC2086 # one pid per word
    kill $pids 2>/dev/null || true
    while [ "$waited" -lt 10 ] && pgrep -f "$pattern" >/dev/null 2>&1; do
        sleep 0.5
        waited=$((waited + 1))
    done
    pids="$(pgrep -f "$pattern" 2>/dev/null || true)"
    if [ -n "$pids" ]; then
        # shellcheck disable=SC2086
        kill -9 $pids 2>/dev/null || true
    fi
}

# --- Where Local may write -----------------------------------------------------

# The absolute path with `.`, `..` and every symlink resolved, whether or not
# it exists yet (what is missing is taken as written: it cannot be a link).
# Fails on a symlink loop. This is the path an install or removal acts on, so
# a `..` or a link in --prefix cannot hide where it lands.
canonicalize_path() {
    local rest="$1" cur="/" comp next target links=0
    case "$rest" in
        /*) ;;
        *) rest="$PWD/$rest" ;;
    esac
    while :; do
        while [ "${rest#/}" != "$rest" ]; do rest="${rest#/}"; done
        [ -n "$rest" ] || break
        comp="${rest%%/*}"
        if [ "$comp" = "$rest" ]; then rest=""; else rest="${rest#*/}"; fi
        case "$comp" in
            .) continue ;;
            ..) cur="$(dirname "$cur")"; continue ;;
        esac
        next="${cur%/}/$comp"
        if [ -L "$next" ]; then
            links=$((links + 1))
            [ "$links" -le 40 ] || return 1
            target="$(readlink "$next")" || return 1
            case "$target" in
                /*) cur="/" ;;
            esac
            rest="$target/$rest"
            continue
        fi
        cur="$next"
    done
    printf '%s' "$cur"
}

# A path as compared: lower case (the usual macOS volume ignores case) and
# without the firmlink prefix that makes /System/Volumes/Data/Applications the
# same folder as /Applications.
path_key() {
    local key
    key="$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')"
    case "$key" in
        /system/volumes/data/*) key="${key#/system/volumes/data}" ;;
    esac
    printf '%s' "$key"
}

# Succeeds when `candidate`, or a folder above it, is the very file system
# object `container` (same device and inode): it is `container` or inside it,
# however the path is spelled.
is_inside_by_identity() {
    local container="$1" candidate="$2" want have
    want="$(stat -f '%d:%i' "$container" 2>/dev/null || true)"
    [ -n "$want" ] || return 1
    while :; do
        have="$(stat -f '%d:%i' "$candidate" 2>/dev/null || true)"
        [ "$have" != "$want" ] || return 0
        [ "$candidate" != "/" ] || return 1
        candidate="$(dirname "$candidate")"
    done
}

# Succeeds when one path is the other or inside it (both given canonical).
paths_overlap() {
    local a b
    a="$(path_key "$1")/"
    b="$(path_key "$2")/"
    case "$a" in "$b"*) return 0 ;; esac
    case "$b" in "$a"*) return 0 ;; esac
    is_inside_by_identity "$1" "$2" && return 0
    is_inside_by_identity "$2" "$1" && return 0
    return 1
}

# What belongs to the released Cua Spaces, one path per line, for the home
# `$1` (canonical): the app, the home, the CLI, Lume's folder, Cua Volume and
# the app's support folder.
stable_owned_paths() {
    local home="$1"
    printf '%s\n' \
        "$STABLE_APP_PARENT/$STABLE_APP_DIR_NAME" \
        "$home/$STABLE_HOME_DIR_NAME" \
        "$home/.local/bin/$STABLE_CLI_NAME" \
        "$home/$STABLE_LUME_DIR_NAME" \
        "$home/$STABLE_VOLUME_NAME" \
        "$home/Library/Application Support/$STABLE_BUNDLE_ID"
}

# Fails, saying why, when `path` (canonical; `label` names it) is or is inside
# anything the released Cua Spaces owns, or inside /Applications, where
# Cua Spaces Local never goes. Each owned path is resolved the same way as
# `path` before they are compared, and checked as written too: ~/.lume may be a
# symlink to /Volumes/VMs, and then /Volumes/VMs is the Stable Lume folder.
# (The identity check below uses lstat, so it cannot see through a link itself.)
reject_stable_overlap() {
    local label="$1" path="$2" home owned resolved
    home="$(canonicalize_path "$HOME")" || { err "cannot resolve HOME ($HOME)"; return 1; }
    while IFS= read -r owned; do
        resolved="$(canonicalize_path "$owned")" || { err "refusing: cannot resolve $owned (a symlink loop?), which belongs to the released Cua Spaces."; return 1; }
        if paths_overlap "$path" "$owned" || paths_overlap "$path" "$resolved"; then
            if [ "$resolved" != "$owned" ]; then owned="$owned (a link to $resolved)"; fi
            err "refusing: the $label path $path is, or is inside, $owned, which belongs to the released Cua Spaces. Pick another --prefix."
            return 1
        fi
    done < <(stable_owned_paths "$home")
    resolved="$(canonicalize_path "$STABLE_APP_PARENT")" || resolved="$STABLE_APP_PARENT"
    if paths_overlap "$path" "$STABLE_APP_PARENT" || paths_overlap "$path" "$resolved"; then
        err "refusing: the $label path $path is inside $STABLE_APP_PARENT, where the released Cua Spaces lives. Cua Spaces Local goes under ~/Applications; pick another --prefix."
        return 1
    fi
}
