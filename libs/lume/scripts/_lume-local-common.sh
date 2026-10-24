#!/bin/bash
# Shared by install-local.sh and uninstall-local.sh (sourced, not run).
#
# The path guard. Both scripts write or delete paths that come from the
# environment or the command line (LUME_LOCAL_HOME, --install-dir, ...). A path
# like "$HOME/./.lume", "$HOME/x/../.lume" or a symlink to ~/.lume names the
# released Lume's VMs without matching them as a string, so every target is
# canonicalised first (`.` and `..` resolved, every symlink followed) and then
# compared with the paths the released Lume owns. A target that equals one of
# them, sits inside one, or contains one is refused.

# Print the canonical form of the absolute path $1: `.` and `..` resolved, and
# every symlink followed, including those below the deepest component that
# exists. Components that do not exist yet are kept as written. Returns 1 when
# $1 is not absolute or has a symlink loop.
lume_local_canon() {
  local queue="$1" resolved="" comp target links=0
  case "$queue" in
    /*) ;;
    *) return 1 ;;
  esac
  while :; do
    while [ "${queue#/}" != "$queue" ]; do queue="${queue#/}"; done
    [ -z "$queue" ] && break
    comp="${queue%%/*}"
    if [ "$comp" = "$queue" ]; then
      queue=""
    else
      queue="${queue#*/}"
    fi
    case "$comp" in
      .) continue ;;
      ..)
        resolved="${resolved%/*}"
        continue
        ;;
    esac
    if [ -L "$resolved/$comp" ]; then
      links=$((links + 1))
      [ "$links" -gt 40 ] && return 1
      target="$(readlink "$resolved/$comp")" || return 1
      case "$target" in
        /*) resolved="" ;;
      esac
      queue="$target/$queue"
      continue
    fi
    resolved="$resolved/$comp"
  done
  printf '%s\n' "${resolved:-/}"
}

# Lower-case $1. APFS is case-insensitive by default, so "$HOME/.LUME" is
# "$HOME/.lume"; comparing lower-cased also refuses it on a case-sensitive
# volume, where it would be a different directory nobody needs.
lume_local_lower() {
  printf '%s' "$1" | tr '[:upper:]' '[:lower:]'
}

# The paths the released Lume owns, one per line, canonical. Needs HOME.
lume_local_stable_paths() {
  local p
  for p in \
    "$HOME/.lume" \
    "$HOME/.config/lume" \
    "$HOME/.local/bin/lume" \
    "$HOME/.local/bin/lume-daemon" \
    "$HOME/.local/bin/lume-update" \
    "$HOME/.local/bin/lume_lume.bundle" \
    "$HOME/.local/share/lume" \
    "$HOME/Library/LaunchAgents/com.trycua.lume_daemon.plist" \
    "$HOME/Library/LaunchAgents/com.trycua.lume_updater.plist" \
    "/tmp/lume_daemon.log" \
    "/tmp/lume_daemon.error.log" \
    "/tmp/lume_updater.log" \
    "/tmp/lume_updater.error.log"; do
    lume_local_canon "$p" || printf '%s\n' "$p"
  done
  if [ -n "${XDG_CONFIG_HOME:-}" ]; then
    case "$XDG_CONFIG_HOME" in
      /*) lume_local_canon "$XDG_CONFIG_HOME/lume" || true ;;
    esac
  fi
  if [ -n "${LOG_DIR:-}" ]; then
    case "$LOG_DIR" in
      /*)
        lume_local_canon "$LOG_DIR/lume_daemon.log" || true
        lume_local_canon "$LOG_DIR/lume_daemon.error.log" || true
        ;;
    esac
  fi
}

# Whether canonical path $1 equals $2 or is inside it (both lower-cased).
lume_local_within() {
  [ "$2" = "/" ] && return 0
  [ "$1" = "$2" ] && return 0
  case "$1" in
    "$2"/*) return 0 ;;
  esac
  return 1
}

# lume_local_check LABEL PATH: print the canonical form of PATH and return 0 if
# it is safe to write or delete; else print why on stderr and return 2.
# Safe means: absolute, no symlink loop, and not equal to, inside, or an
# ancestor of anything the released Lume owns.
lume_local_check() {
  local label="$1" raw="$2" canon canon_lc i
  local stable_list stable_lc_list
  case "$raw" in
    /*) ;;
    *)
      echo "${RED}Error: $label must be an absolute path, got '$raw'.${NORMAL}" >&2
      return 2
      ;;
  esac
  if ! canon="$(lume_local_canon "$raw")"; then
    echo "${RED}Error: cannot resolve $label '$raw' (symlink loop?).${NORMAL}" >&2
    return 2
  fi
  canon_lc="$(lume_local_lower "$canon")"
  # The released Lume's paths, computed once per run, with a lower-cased copy.
  if [ -z "${LUME_LOCAL_STABLE_PATHS:-}" ]; then
    LUME_LOCAL_STABLE_PATHS="$(lume_local_stable_paths)"
    LUME_LOCAL_STABLE_PATHS_LC="$(lume_local_lower "$LUME_LOCAL_STABLE_PATHS")"
  fi
  local old_ifs="$IFS"
  IFS='
'
  set -f
  stable_list=($LUME_LOCAL_STABLE_PATHS)
  stable_lc_list=($LUME_LOCAL_STABLE_PATHS_LC)
  set +f
  IFS="$old_ifs"
  i=0
  while [ "$i" -lt "${#stable_list[@]}" ]; do
    if lume_local_within "$canon_lc" "${stable_lc_list[$i]}" \
      || lume_local_within "${stable_lc_list[$i]}" "$canon_lc"; then
      echo "${RED}Error: refusing $label '$raw'${NORMAL}" >&2
      echo "${RED}  It resolves to $canon, which is, is inside, or contains ${stable_list[$i]}, a path the released Lume uses.${NORMAL}" >&2
      return 2
    fi
    i=$((i + 1))
  done
  printf '%s\n' "$canon"
}
