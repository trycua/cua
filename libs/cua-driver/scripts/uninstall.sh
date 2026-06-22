#!/usr/bin/env bash
# cua-driver uninstaller (Rust implementation by default, retired Swift
# driver as an explicit legacy path on macOS). Mirrors uninstall.ps1 on
# Windows: one canonical script per shell, no private `_uninstall-rust.sh`
# helper.
#
# Behaviour by host + flag:
#   macOS/Linux + no flag           → Rust uninstall
#   macOS  + --backend=swift        → retired Swift uninstall
#   macOS  + --experimental-rust    → same as no flag (legacy alias)
#   macOS  + --backend=rust         → same as no flag
#   Linux/other + --backend=swift   → no-op (still allowed for compatibility)
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
#   Linux:
#     - ~/.local/bin/cua-driver symlink (only when it resolves to a
#       cua-driver path — a Swift-driver symlink is left in place)
#     - ~/.cua-driver/ (current package home) + legacy ~/.cua-driver-rs/
#       (telemetry id, config.json, versioned releases, current symlink)
#     - ~/.config/systemd/user/cua-driver-rs.service (if --autostart
#       was used via install-local.sh — stop + disable + remove)
#     - Skill symlinks under ~/.claude/skills/cua-driver(-rs), ~/.agents/
#       skills/…, ~/.openclaw/skills/…, ~/.config/opencode/skills/…
#   macOS:
#     - /Applications/CuaDriver.app bundle (+ legacy CuaDriverRs.app)
#     - ~/.local/bin/cua-driver symlink (only when it resolves into
#       /Applications/CuaDriver.app)
#     - ~/.cua-driver/ (current package home) + legacy ~/.cua-driver-rs/
#     - ~/Library/LaunchAgents/com.trycua.cua-driver-rs.plist (if
#       --autostart was used via install-local.sh — unload + remove)
#     - Skill symlinks under ~/.claude/skills/cua-driver(-rs), etc.
#
# Shared-path safety: /Applications/CuaDriver.app + its ~/.local/bin
# symlink use the same bundle id (com.trycua.driver) as the Swift driver,
# so they're only removed when an unambiguous Rust marker is on disk
# (~/.cua-driver/packages/, legacy ~/.cua-driver-rs/, CuaDriverRs.app,
# or the LaunchAgent/systemd unit).
#
# Also scrubs Claude MCP registrations in ~/.claude.json that match
# the active backend.
#
# Does NOT revoke TCC grants on macOS (Accessibility + Screen Recording).
# The closing message points at the tccutil commands for a clean
# re-install flow.
#
# Usage:
#   /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver/scripts/uninstall.sh)"
#
# Env overrides (mirror install side):
#   CUA_DRIVER_RS_HOME    Rust package home to remove (default ~/.cua-driver-rs)
set -euo pipefail

# ----------------------------------------------------------------------
# Flag parsing — same two-pass shape as install.sh so the argv shapes
# stay bit-compatible across install/uninstall and a future Rust-only
# flag flows through without edits.
# ----------------------------------------------------------------------
USE_RUST_BACKEND=1
RESET_TCC=0
FORWARDED_ARGS=()
PASSTHROUGH=0
while [[ $# -gt 0 ]]; do
    if [[ "$PASSTHROUGH" == "1" ]]; then
        FORWARDED_ARGS+=("$1"); shift; continue
    fi
    case "$1" in
        --experimental-rust) shift ;;  # legacy alias for default Rust path
        --backend=rust)      shift ;;
        --backend=swift)     USE_RUST_BACKEND=0; shift ;;
        --reset-tcc)         RESET_TCC=1; shift ;;  # also revoke TCC grants (opt-in)
        --backend=*)
            printf 'error: unknown backend %q; supported: swift, rust\n' "${1#*=}" >&2
            exit 2
            ;;
        --)                  PASSTHROUGH=1; shift ;;  # forward the rest verbatim
        *)                   FORWARDED_ARGS+=("$1"); shift ;;
    esac
done

# The Swift binary is macOS-only, so `--backend=swift` on Linux is a
# deliberate no-op (the script reaches the Swift branch below, finds
# nothing, exits clean).
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

# Opt-in TCC revocation (`--reset-tcc`). Off by default: the bundle id
# com.trycua.driver is shared with the retired Swift driver, and keeping
# grants across a reinstall avoids a re-prompt — so wiping privacy state
# is a deliberate, explicit choice, not a side effect of uninstall.
# When the flag is set, revoke Accessibility + Screen-Recording +
# Automation for com.trycua.driver. macOS-only; no-op elsewhere.
maybe_reset_tcc() {
    [[ "$RESET_TCC" == "1" ]] || return 0
    if [[ "$OS" != "Darwin" ]]; then
        log "--reset-tcc is macOS-only; nothing to revoke on $OS"
        return 0
    fi
    if ! command -v tccutil >/dev/null 2>&1; then
        log "--reset-tcc: tccutil not found; skipping"
        return 0
    fi
    log "revoking TCC grants for com.trycua.driver (--reset-tcc)"
    log "  note: com.trycua.driver is shared with the retired Swift driver;"
    log "  this clears grants for both. The next launch will re-prompt."
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
    LAUNCHAGENT_PLIST="$HOME/Library/LaunchAgents/com.trycua.cua-driver-rs.plist"
    SYSTEMD_USER_UNIT="$HOME/.config/systemd/user/cua-driver-rs.service"
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
    #   - LaunchAgent plist / systemd unit exists (autostart was used)
    RUST_INSTALL_PRESENT=0
    if [[ -d "$PACKAGES_DIR" || -d "$LEGACY_HOME_DIR" || -d "$LEGACY_APP_BUNDLE" || -f "$LAUNCHAGENT_PLIST" || -f "$SYSTEMD_USER_UNIT" ]]; then
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
    # ~/.config/systemd/user/cua-driver-rs.service. Stop + disable +
    # remove if present so the daemon doesn't come back at next logon.
    # systemctl --user no-ops gracefully on a non-systemd host.
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

    # --- Autostart (macOS LaunchAgent) ---
    # install-local.sh --autostart on macOS registers
    # ~/Library/LaunchAgents/com.trycua.cua-driver-rs.plist. Unload (so
    # the running daemon stops) + remove the plist.
    if [[ "$OS" == "Darwin" && -f "$LAUNCHAGENT_PLIST" ]]; then
        launchctl unload "$LAUNCHAGENT_PLIST" 2>/dev/null || true
        rm -f "$LAUNCHAGENT_PLIST"
        log "removed LaunchAgent $LAUNCHAGENT_PLIST"
    elif [[ "$OS" == "Darwin" ]]; then
        log "no LaunchAgent at $LAUNCHAGENT_PLIST (skipping)"
    fi

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
                log "$APP_BUNDLE exists but no Rust marker on disk (~/.cua-driver/packages/, ~/.cua-driver-rs/, CuaDriverRs.app, LaunchAgent, systemd unit); leaving it (looks like a Swift-only install)"
            fi
        else
            log "no app bundle at $APP_BUNDLE (skipping)"
        fi
    fi

    # --- Package home ---
    # Everything under $HOME_DIR (default ~/.cua-driver): telemetry id,
    # config.json, versioned releases, current symlink, local skill copy,
    # version_check.json cache. This is the live runtime home now (post
    # v0.2.16 rename), so gate its removal on the Rust marker — don't nuke
    # a Swift-only Mac's ~/.cua-driver/config.json on a mistaken run.
    if [[ -d "$HOME_DIR" ]]; then
        if [[ "$RUST_INSTALL_PRESENT" == "1" ]]; then
            rm -rf "$HOME_DIR"
            log "removed $HOME_DIR"
        else
            log "$HOME_DIR exists but no Rust marker on disk; leaving it (looks like a Swift-only / shared config dir)"
        fi
    else
        log "no package home at $HOME_DIR (skipping)"
    fi
    # Legacy pre-rename home is unambiguously Rust — always sweep it.
    if [[ -d "$LEGACY_HOME_DIR" ]]; then
        rm -rf "$LEGACY_HOME_DIR"
        log "removed legacy package home $LEGACY_HOME_DIR"
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
    maybe_reset_tcc
    if [[ "$OS" == "Darwin" ]]; then
        echo ""
        echo "cua-driver uninstalled."
        if [[ "$RESET_TCC" != "1" ]]; then
            cat << 'FINALUNMSG'

TCC grants (Accessibility + Screen Recording) remain in System
Settings > Privacy & Security. Reset them explicitly — or re-run with
--reset-tcc — if you want a clean re-install flow:

  tccutil reset Accessibility com.trycua.driver
  tccutil reset ScreenCapture com.trycua.driver
FINALUNMSG
        fi
    else
        cat << 'FINALUNMSG'

cua-driver uninstalled.
FINALUNMSG
    fi
    exit 0
fi

# ----------------------------------------------------------------------
# Swift uninstall branch (macOS legacy, explicit --backend=swift only).
# ----------------------------------------------------------------------
if [[ "$OS" != "Darwin" ]]; then
    log "legacy Swift uninstall requested on non-macOS host ($OS); nothing to remove"
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
Settings > Privacy & Security. Reset them explicitly — or re-run with
--reset-tcc — if you want a clean re-install flow:

  tccutil reset Accessibility com.trycua.driver
  tccutil reset ScreenCapture com.trycua.driver
FINALUNMSG
fi
