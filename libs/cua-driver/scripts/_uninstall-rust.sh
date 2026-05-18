#!/usr/bin/env bash
# _uninstall-rust.sh — private helper invoked by libs/cua-driver/scripts/uninstall.sh
# (the canonical user-facing uninstaller) when --backend=rust is selected or
# the script auto-detects a non-macOS host. Not intended for direct
# invocation; user-facing one-liners always go through the parent
# uninstall.sh, which forwards args + handles backend selection.
#
# Mirrors _install-rust.sh's on-disk knowledge — removes everything the
# install script laid down:
#
# Linux:
#   - ~/.local/bin/cua-driver symlink (only when it resolves to a
#     cua-driver-rs path — a Swift-driver symlink is left in place)
#   - ~/.cua-driver-rs/ (entire package home: telemetry id, install
#     marker, versioned releases, current symlink, lockfile)
#   - ~/.config/systemd/user/cua-driver-rs.service (if --autostart was
#     used via install-local.sh — stop + disable + remove)
#   - Skill symlinks under ~/.claude/skills/cua-driver-rs, ~/.agents/
#     skills/cua-driver-rs, ~/.openclaw/skills/cua-driver-rs,
#     ~/.config/opencode/skills/cua-driver-rs
#
# macOS:
#   - /Applications/CuaDriver.app bundle (and legacy CuaDriverRs.app if present)
#   - ~/.local/bin/cua-driver symlink (only when it resolves into
#     /Applications/CuaDriver.app — see note on the bundle-id share below)
#   - ~/.cua-driver-rs/ (entire package home)
#   - ~/Library/LaunchAgents/com.trycua.cua-driver-rs.plist (if
#     --autostart was used via install-local.sh — unload + remove)
#   - Skill symlinks under ~/.claude/skills/cua-driver-rs, etc.
#
# Also scrubs Claude MCP registrations in ~/.claude.json that point at
# the cua-driver-rs binary.
#
# Does NOT revoke TCC grants on macOS — same conservative stance as the
# Swift uninstall.sh. The closing message points at the tccutil commands
# for a clean re-install flow.
set -euo pipefail

USER_BIN_LINK="$HOME/.local/bin/cua-driver"
# Canonical bundle path (post-rename — shares bundle id `com.trycua.driver`
# with the Swift driver). The Rust install replaces Swift here; both
# the Rust and Swift uninstallers target this path.
APP_BUNDLE="/Applications/CuaDriver.app"
# Legacy bundle path from earlier Rust releases that coexisted with
# Swift under a separate name. Cleaned up if found.
LEGACY_APP_BUNDLE="/Applications/CuaDriverRs.app"
HOME_DIR="${CUA_DRIVER_RS_HOME:-$HOME/.cua-driver-rs}"
LAUNCHAGENT_PLIST="$HOME/Library/LaunchAgents/com.trycua.cua-driver-rs.plist"
SYSTEMD_USER_UNIT="$HOME/.config/systemd/user/cua-driver-rs.service"
SKILL_PACK_NAME="cua-driver-rs"

OS="$(uname -s 2>/dev/null || echo unknown)"

log() { printf '==> %s\n' "$*"; }

# Resolve a symlink target to an absolute path. realpath -e fails when the
# target is missing — we want to inspect dangling symlinks too (a leftover
# from a half-removed install should still be cleaned up here), so fall
# back to readlink + manual normalize when realpath errors out.
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
    # Dangling symlink — readlink still returns the stored target. Make
    # it absolute by joining with the link's dirname for non-absolute
    # stored values.
    target="$(readlink "$link" 2>/dev/null || true)"
    case "$target" in
        /*) printf '%s' "$target" ;;
        *)  printf '%s/%s' "$(cd -- "$(dirname -- "$link")" && pwd)" "$target" ;;
    esac
}

# --- CLI symlink ---------------------------------------------------------
#
# Only remove ~/.local/bin/cua-driver when it resolves into a cua-driver-rs
# install. Post-rename, the Rust install lives at /Applications/CuaDriver.app
# — the SAME path the Swift driver uses, with the same bundle id
# `com.trycua.driver`. Path-based detection alone can't distinguish
# them. We rely on the presence of `$HOME_DIR` (~/.cua-driver-rs/) — the
# Rust-specific state dir — as the marker that this is a Rust install.
# Pre-rename installs at /Applications/CuaDriverRs.app are still cleaned
# up unambiguously by path.
if [[ -L "$USER_BIN_LINK" ]]; then
    RESOLVED="$(resolve_link "$USER_BIN_LINK")"
    case "$RESOLVED" in
        *"CuaDriverRs.app"*|*"/Applications/CuaDriver.app"*|*"$HOME_DIR"*|*".cua-driver-rs"*)
            rm -f "$USER_BIN_LINK"
            log "removed $USER_BIN_LINK -> $RESOLVED"
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

# --- Autostart (Linux systemd --user) -----------------------------------
#
# install-local.sh --autostart registers ~/.config/systemd/user/
# cua-driver-rs.service. Stop + disable + remove if present so the
# daemon doesn't come back at the next logon. systemctl --user no-ops
# gracefully on a non-systemd host (returns non-zero, which we swallow).
if [[ "$OS" == "Linux" && -f "$SYSTEMD_USER_UNIT" ]]; then
    if command -v systemctl >/dev/null 2>&1; then
        systemctl --user stop    cua-driver-rs.service 2>/dev/null || true
        systemctl --user disable cua-driver-rs.service 2>/dev/null || true
        log "stopped + disabled systemd --user unit cua-driver-rs.service"
    fi
    rm -f "$SYSTEMD_USER_UNIT"
    log "removed $SYSTEMD_USER_UNIT"
    if command -v systemctl >/dev/null 2>&1; then
        systemctl --user daemon-reload 2>/dev/null || true
    fi
elif [[ "$OS" == "Linux" ]]; then
    log "no systemd --user unit at $SYSTEMD_USER_UNIT (skipping)"
fi

# --- Autostart (macOS LaunchAgent) --------------------------------------
#
# install-local.sh --autostart on macOS registers
# ~/Library/LaunchAgents/com.trycua.cua-driver-rs.plist. Unload (so the
# running daemon stops) + remove the plist.
if [[ "$OS" == "Darwin" && -f "$LAUNCHAGENT_PLIST" ]]; then
    launchctl unload "$LAUNCHAGENT_PLIST" 2>/dev/null || true
    rm -f "$LAUNCHAGENT_PLIST"
    log "removed LaunchAgent $LAUNCHAGENT_PLIST"
elif [[ "$OS" == "Darwin" ]]; then
    log "no LaunchAgent at $LAUNCHAGENT_PLIST (skipping)"
fi

# --- .app bundle (macOS only) -------------------------------------------
#
# Removes /Applications/CuaDriver.app (current canonical Rust path,
# shared with the Swift driver via bundle id `com.trycua.driver`) AND
# /Applications/CuaDriverRs.app (legacy, pre-rename releases).
# Removing the shared `.app` path is correct here: a user who runs
# `uninstall.sh --experimental-rust` wants the binary on disk gone;
# if they previously had the Swift binary at the same path, they've
# already overwritten it with Rust on install. Re-installing the
# Swift driver afterward is a re-run of the canonical `install.sh`.
if [[ "$OS" == "Darwin" ]]; then
    for bundle_path in "$APP_BUNDLE" "$LEGACY_APP_BUNDLE"; do
        if [[ -d "$bundle_path" ]]; then
            SUDO=""
            if [[ ! -w "$(dirname "$bundle_path")" ]]; then
                SUDO="sudo"
            fi
            $SUDO rm -rf "$bundle_path"
            log "removed $bundle_path"
        else
            log "no app bundle at $bundle_path (skipping)"
        fi
    done
fi

# --- Package home -------------------------------------------------------
#
# Everything under $CUA_DRIVER_RS_HOME (default ~/.cua-driver-rs):
# telemetry id, install marker, versioned releases, current symlink,
# lockfile, local skill copy, version_check.json cache.
if [[ -d "$HOME_DIR" ]]; then
    rm -rf "$HOME_DIR"
    log "removed $HOME_DIR"
else
    log "no package home at $HOME_DIR (skipping)"
fi

# --- Agent skill symlinks -----------------------------------------------
#
# Only remove when the link is a symlink — never clobber a real
# directory (a dev user with a hand-managed skills dir is safe). Note
# we don't check the target here because `cua-driver skills install`
# writes platform-dependent targets (the local copy under $HOME_DIR/
# skills/cua-driver-rs/), and on Windows the `<HomeDir>/skills` shape
# is different. The [[ -L ]] check is the load-bearing safety bar.
for SKILL_LINK in \
    "$HOME/.claude/skills/$SKILL_PACK_NAME" \
    "$HOME/.agents/skills/$SKILL_PACK_NAME" \
    "$HOME/.openclaw/skills/$SKILL_PACK_NAME" \
    "$HOME/.config/opencode/skills/$SKILL_PACK_NAME"; do
    if [[ -L "$SKILL_LINK" ]]; then
        rm -f "$SKILL_LINK"
        log "removed skill symlink $SKILL_LINK"
    elif [[ -d "$SKILL_LINK" ]]; then
        log "$SKILL_LINK is a real directory, not a symlink (skipping)"
    else
        log "no skill symlink at $SKILL_LINK (skipping)"
    fi
done

# --- Claude Code MCP registrations --------------------------------------
#
# Same scrub shape as the Swift uninstall.sh, keyed on the cua-driver-rs
# binary name + the per-platform install paths. Unrelated MCP servers
# are left alone.
CLAUDE_JSON="$HOME/.claude.json"
if [[ -f "$CLAUDE_JSON" ]] && command -v python3 >/dev/null 2>&1; then
    PY_OUTPUT="$(
        CLAUDE_JSON="$CLAUDE_JSON" HOME_DIR="$HOME_DIR" python3 <<'PY'
import json
import os
import shutil
import sys
import tempfile
import time

path = os.environ["CLAUDE_JSON"]
home_dir = os.environ.get("HOME_DIR", "")

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
    # ambiguous (the Swift binary uses the same filename), so we key on
    # the Rust-specific paths here. The user can run the Swift
    # uninstall.sh separately if they have both.
    if home_dir and home_dir in joined:
        return True
    return ("CuaDriverRs.app" in joined
            or "/Applications/CuaDriver.app" in joined
            or ".cua-driver-rs" in joined
            or "cua-driver-rs" in joined)

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

# Best-effort CLI cleanup. `claude mcp remove` only touches the active
# project / user scopes — fine to run; it's a no-op when the entries
# were already scrubbed above.
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

# --- Closing message ----------------------------------------------------

if [[ "$OS" == "Darwin" ]]; then
    cat << 'FINALUNMSG'

cua-driver-rs uninstalled.

TCC grants (Accessibility + Screen Recording) remain in System
Settings > Privacy & Security. Reset them explicitly if you want a
clean re-install flow:

  tccutil reset Accessibility com.trycua.driver
  tccutil reset ScreenCapture com.trycua.driver

  (Note: `com.trycua.driver` is shared with the Swift cua-driver.
  Resetting it clears grants for both backends. If you still use the
  Swift driver, skip this step and let macOS keep the grants — the
  next Swift launch will re-use them.)
FINALUNMSG
else
    cat << 'FINALUNMSG'

cua-driver-rs uninstalled.
FINALUNMSG
fi
