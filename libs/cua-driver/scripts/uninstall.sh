#!/usr/bin/env bash
# cua-driver uninstaller. Removes everything install.sh laid down — Swift
# driver by default, cua-driver-rs (Rust port) when --experimental-rust /
# --backend=rust is passed (mirrors install.sh's flag set), or auto-detects
# the Rust port on non-macOS hosts (same as install.sh).
#
# Swift uninstall removes:
#   - ~/.local/bin/cua-driver symlink
#   - /Applications/CuaDriver.app bundle
#   - ~/.cua-driver/ (telemetry id + install marker)
#   - ~/Library/Application Support/Cua Driver/ (config.json)
#   - ~/Library/Caches/cua-driver/ (daemon/cache state)
#
# Rust uninstall (--experimental-rust / --backend=rust / non-macOS) delegates
# to a colocated private helper, _uninstall-rust.sh — see that script for
# the full list of paths it removes.
#
# Does NOT revoke TCC grants (Accessibility + Screen Recording) on macOS.
#
# Flags:
#   --experimental-rust  uninstall the cua-driver-rs (Rust port) backend
#                        instead of the Swift binary. Delegates to
#                        libs/cua-driver/scripts/_uninstall-rust.sh. The
#                        Swift binary (if present) is left untouched.
#                        Also accepted as --backend=rust.
#   --backend=swift      explicit no-op default (Swift uninstall).
#
# Usage:
#   /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver/scripts/uninstall.sh)"
#
#   # cua-driver-rs (Rust port) — explicit opt-in on macOS:
#   /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver/scripts/uninstall.sh)" -- --experimental-rust
#
#   # Linux auto-detects and removes the Rust port without any flag.
set -euo pipefail

# Rust-backend delegation target. The Rust uninstall logic is a private
# helper script colocated with this one — _uninstall-rust.sh — so that
# this directory holds the single user-facing uninstall.sh per platform.
# `--experimental-rust` below either execs the on-disk helper (dev /
# checked-out-tree case) or curls this URL and pipes it to bash
# (`curl ... | bash` uninstall case). Mirrors install.sh's pattern.
RUST_UNINSTALLER_URL="https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver/scripts/_uninstall-rust.sh"

# Lightweight flag parsing — same two-pass shape as install.sh so the
# argv shapes stay bit-compatible across install/uninstall and a future
# Rust-only flag added to _uninstall-rust.sh flows through here without
# edits to this file.
USE_RUST_BACKEND=0
FORWARDED_ARGS=()
PASSTHROUGH=0
while [[ $# -gt 0 ]]; do
    if [[ "$PASSTHROUGH" == "1" ]]; then
        FORWARDED_ARGS+=("$1"); shift; continue
    fi
    case "$1" in
        --experimental-rust) USE_RUST_BACKEND=1; shift ;;
        --backend=rust)      USE_RUST_BACKEND=1; shift ;;
        --backend=swift)     shift ;;                 # explicit default — no-op
        --backend=*)
            printf 'error: unknown backend %q; supported: swift, rust\n' "${1#*=}" >&2
            exit 2
            ;;
        --)                  PASSTHROUGH=1; shift ;;  # forward the rest verbatim
        *)                   FORWARDED_ARGS+=("$1"); shift ;;
    esac
done

# --- Auto-delegate to Rust on non-macOS ---------------------------------
#
# The Swift binary is macOS-only — there's nothing for the Swift uninstall
# path to remove on Linux. Auto-set USE_RUST_BACKEND=1 so a single
# canonical URL works on every platform: `curl … cua-driver/scripts/
# uninstall.sh | bash` removes the Swift driver on macOS (today's default)
# and the Rust port on Linux. macOS users who want to remove the Rust
# port still pass `--experimental-rust` / `--backend=rust` explicitly.
AUTO_RUST=0
if [[ "$USE_RUST_BACKEND" == "0" && "$(uname -s 2>/dev/null)" != "Darwin" ]]; then
    USE_RUST_BACKEND=1
    AUTO_RUST=1
    printf 'note: detected non-macOS host (%s); auto-selecting the cua-driver-rs Rust uninstall.\n' \
        "$(uname -s 2>/dev/null || echo unknown)" >&2
    printf '      Pass --backend=swift to force the Swift uninstall path (will be a no-op on non-Darwin).\n' >&2
fi

# --- Optional delegation to the experimental Rust backend ---------------
#
# If the user opted in with --experimental-rust / --backend=rust, hand the
# rest of argv to _uninstall-rust.sh and exit. The Swift uninstall path
# below is never touched in this case, so the Swift binary (if present)
# is left exactly as-is.
if [[ "$USE_RUST_BACKEND" == "1" ]]; then
    if [[ "$AUTO_RUST" == "0" ]]; then
        # Explicit opt-in on macOS.
        printf 'note: uninstalling cua-driver-rs (Rust port). The Swift binary won'"'"'t be touched.\n' >&2
    else
        # Auto-selected on Linux/other — drop the "experimental" qualifier
        # (Rust is the canonical install path on every non-macOS platform).
        printf 'note: uninstalling cua-driver-rs (the Rust port — canonical on non-macOS).\n' >&2
    fi

    # Prefer the on-disk copy when this script is running from a checked-out
    # tree (dev / CI). Falls back to curling the canonical URL for the
    # `curl ... | bash` uninstall path, where $BASH_SOURCE is unset / -.
    LOCAL_RUST_UNINSTALLER=""
    if [[ -n "${BASH_SOURCE[0]:-}" && "${BASH_SOURCE[0]}" != "-" && -f "${BASH_SOURCE[0]}" ]]; then
        SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
        CANDIDATE="$SCRIPT_DIR/_uninstall-rust.sh"
        if [[ -f "$CANDIDATE" ]]; then
            LOCAL_RUST_UNINSTALLER="$CANDIDATE"
        fi
    fi

    # macOS ships bash 3.2, which trips `set -u` when expanding an empty
    # array via "${arr[@]}" — guard with the +alt-value pattern so the
    # zero-arg case becomes a literal no-expansion.
    if [[ -n "$LOCAL_RUST_UNINSTALLER" ]]; then
        exec /bin/bash "$LOCAL_RUST_UNINSTALLER" ${FORWARDED_ARGS[@]+"${FORWARDED_ARGS[@]}"}
    else
        if ! command -v curl >/dev/null 2>&1; then
            printf 'error: curl not found on PATH; cannot fetch %s\n' "$RUST_UNINSTALLER_URL" >&2
            exit 1
        fi
        # `exec` so the Rust uninstaller replaces this process — we don't
        # want to fall through to the Swift uninstall path on any error here.
        RUST_UNINSTALLER_SCRIPT="$(curl -fsSL "$RUST_UNINSTALLER_URL")" || {
            printf 'error: failed to download Rust uninstaller from %s\n' "$RUST_UNINSTALLER_URL" >&2
            exit 1
        }
        exec /bin/bash -c "$RUST_UNINSTALLER_SCRIPT" cua-driver-rs-uninstall ${FORWARDED_ARGS[@]+"${FORWARDED_ARGS[@]}"}
    fi
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

log() { printf '==> %s\n' "$*"; }

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
# is ours — a dev user pointing the symlink at a working copy of the repo
# keeps theirs untouched.
SKILL_TARGET_EXPECTED="$APP_BUNDLE/Contents/Resources/Skills/cua-driver"
for SKILL_LINK in \
    "$HOME/.claude/skills/cua-driver" \
    "$HOME/.agents/skills/cua-driver" \
    "$HOME/.openclaw/skills/cua-driver" \
    "$HOME/.config/opencode/skills/cua-driver"; do
    if [[ -L "$SKILL_LINK" ]] && [[ "$(readlink "$SKILL_LINK")" == "$SKILL_TARGET_EXPECTED" ]]; then
        rm -f "$SKILL_LINK"
        log "removed $SKILL_LINK"
    else
        log "no install-created skill symlink at $SKILL_LINK (skipping)"
    fi
done

# Claude Code MCP registrations. `claude mcp remove` only removes from
# the current project / user scopes, while ~/.claude.json can also contain
# stale project entries for other directories. Scrub only registrations
# that are explicitly named cua-driver or whose command points at a
# cua-driver binary, so unrelated servers named "computer-use" are left
# alone.
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
# .mcp.json / current-working-directory scopes when present and is harmless
# when the entries were already removed above.
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

cat << 'FINALUNMSG'

cua-driver uninstalled.

TCC grants (Accessibility + Screen Recording) remain in System
Settings > Privacy & Security. Reset them explicitly if you want a
clean re-install flow:

  tccutil reset Accessibility com.trycua.driver
  tccutil reset ScreenCapture com.trycua.driver
FINALUNMSG
