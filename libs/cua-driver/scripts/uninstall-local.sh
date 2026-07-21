#!/usr/bin/env bash
# Remove only the source-built cua-driver-local product. The released
# cua-driver installation has different names and paths and is never touched.
set -euo pipefail

RESET_TCC=1
FORCE=0
VALIDATE_ONLY=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --keep-tcc) RESET_TCC=0 ;;
        --reset-tcc) RESET_TCC=1 ;;
        --force) FORCE=1 ;;
        --validate-only) VALIDATE_ONLY=1 ;;
        --help|-h)
            echo "Usage: $0 [--force] [--keep-tcc] [--validate-only]"
            exit 0
            ;;
        *) echo "error: unknown option: $1" >&2; exit 2 ;;
    esac
    shift
done

OS="$(uname -s 2>/dev/null || echo unknown)"
HOME_DIR="${CUA_DRIVER_LOCAL_HOME:-$HOME/.cua-driver-local}"
BIN_DIR="${CUA_DRIVER_LOCAL_INSTALL_DIR:-$HOME/.local/bin}"
CLI_LINK="$BIN_DIR/cua-driver-local"
APP_BUNDLE="/Applications/CuaDriverLocal.app"
if [[ "$OS" == "Darwin" ]]; then
    CACHE_DIR="$HOME/Library/Caches/cua-driver-local"
else
    CACHE_DIR="$HOME/.cache/cua-driver-local"
fi
LAUNCHAGENT="$HOME/Library/LaunchAgents/com.trycua.cua-driver-local.plist"
SYSTEMD_UNIT="$HOME/.config/systemd/user/cua-driver-local.service"

log() { printf '==> %s\n' "$*"; }

case "$HOME_DIR" in
    /*) ;;
    *) echo "error: CUA_DRIVER_LOCAL_HOME must be an absolute path" >&2; exit 2 ;;
esac
if [[ "$HOME_DIR" == "$HOME" || "$HOME_DIR" == "$HOME/.cua-driver" || "$HOME_DIR" == "/" ]]; then
    echo "error: refusing unsafe or release-owned local home: $HOME_DIR" >&2
    exit 2
fi
case "$BIN_DIR" in
    /*) ;;
    *) echo "error: CUA_DRIVER_LOCAL_INSTALL_DIR must be an absolute path" >&2; exit 2 ;;
esac

if [[ "$VALIDATE_ONLY" == "1" ]]; then
    printf 'cli=%s\nhome=%s\ncache=%s\napp=%s\nbundle=com.trycua.driver.local\nlaunchagent=%s\nsystemd=%s\n' \
        "$CLI_LINK" "$HOME_DIR" "$CACHE_DIR" "$APP_BUNDLE" "$LAUNCHAGENT" "$SYSTEMD_UNIT"
    exit 0
fi

if [[ "$FORCE" != "1" ]]; then
    printf 'Remove the local cua-driver identity and its state? [y/N] '
    read -r reply
    case "$reply" in y|Y|yes|YES) ;; *) log "cancelled"; exit 0 ;; esac
fi

resolve_link() {
    local link="$1" target
    [[ -L "$link" ]] || { printf ''; return; }
    if target="$(realpath "$link" 2>/dev/null)"; then
        printf '%s' "$target"
        return
    fi
    target="$(readlink "$link" 2>/dev/null || true)"
    case "$target" in
        /*) printf '%s' "$target" ;;
        *) printf '%s/%s' "$(cd -- "$(dirname -- "$link")" && pwd)" "$target" ;;
    esac
}

is_local_target() {
    case "$1" in
        "$HOME_DIR"/*|"$APP_BUNDLE"/*) return 0 ;;
        *) return 1 ;;
    esac
}

# Stop only local autostart/process identities.
if [[ "$OS" == "Darwin" && -f "$LAUNCHAGENT" ]]; then
    launchctl unload "$LAUNCHAGENT" 2>/dev/null || true
    rm -f "$LAUNCHAGENT"
    log "removed LaunchAgent $LAUNCHAGENT"
elif [[ "$OS" == "Linux" && -f "$SYSTEMD_UNIT" ]]; then
    if command -v systemctl >/dev/null 2>&1; then
        systemctl --user disable --now cua-driver-local.service 2>/dev/null || true
    fi
    rm -f "$SYSTEMD_UNIT"
    log "removed systemd user unit $SYSTEMD_UNIT"
fi
pkill -x cua-driver-local >/dev/null 2>&1 || true

# Revoke only the local bundle's TCC rows, while LaunchServices can resolve it.
if [[ "$OS" == "Darwin" && "$RESET_TCC" == "1" ]] && command -v tccutil >/dev/null 2>&1; then
    if [[ -d "$APP_BUNDLE" ]]; then
        LSREGISTER="/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister"
        [[ ! -x "$LSREGISTER" ]] || "$LSREGISTER" -f "$APP_BUNDLE" >/dev/null 2>&1 || true
    fi
    for service in Accessibility ScreenCapture AppleEvents; do
        tccutil reset "$service" com.trycua.driver.local >/dev/null 2>&1 || true
    done
    log "revoked TCC grants for com.trycua.driver.local"
fi

# Remove the CLI only if it is an installer-created link into the local product.
if [[ -L "$CLI_LINK" ]]; then
    target="$(resolve_link "$CLI_LINK")"
    if is_local_target "$target"; then
        rm -f "$CLI_LINK"
        log "removed $CLI_LINK"
    else
        log "$CLI_LINK points outside the local install; leaving it"
    fi
elif [[ -e "$CLI_LINK" ]]; then
    log "$CLI_LINK is not a symlink; leaving it"
fi

# Skill links use the shared pack name, so target ownership is mandatory.
for skill_link in \
    "$HOME/.claude/skills/cua-driver" \
    "$HOME/.agents/skills/cua-driver" \
    "$HOME/.openclaw/skills/cua-driver" \
    "$HOME/.config/opencode/skills/cua-driver" \
    "$HOME/.gemini/skills/cua-driver" \
    "$HOME/.hermes/skills/cua-driver"; do
    if [[ -L "$skill_link" ]]; then
        target="$(resolve_link "$skill_link")"
        if is_local_target "$target"; then
            rm -f "$skill_link"
            log "removed local skill link $skill_link"
        fi
    fi
done

# Scrub Claude registrations only when their command/args point at local paths.
CLAUDE_JSON="$HOME/.claude.json"
if [[ -f "$CLAUDE_JSON" ]] && command -v python3 >/dev/null 2>&1; then
    CLAUDE_JSON="$CLAUDE_JSON" LOCAL_HOME="$HOME_DIR" LOCAL_APP="$APP_BUNDLE" python3 <<'PY'
import json, os, shutil, tempfile, time

path = os.environ["CLAUDE_JSON"]
try:
    with open(path, encoding="utf-8") as handle:
        data = json.load(handle)
except (OSError, ValueError):
    raise SystemExit(0)

anchors = ("cua-driver-local", os.environ["LOCAL_HOME"], os.environ["LOCAL_APP"])
removed = []

def is_local(server):
    if not isinstance(server, dict):
        return False
    values = [server.get("command", "")]
    args = server.get("args", [])
    values.extend(args if isinstance(args, list) else [args])
    joined = " ".join(value for value in values if isinstance(value, str))
    return any(anchor and anchor in joined for anchor in anchors)

def scrub(servers, scope):
    if not isinstance(servers, dict):
        return
    for name in list(servers):
        if is_local(servers[name]):
            del servers[name]
            removed.append(f"{scope}:{name}")

scrub(data.get("mcpServers"), "user")
for project_name, project in (data.get("projects") or {}).items():
    if isinstance(project, dict):
        scrub(project.get("mcpServers"), f"project:{project_name}")

if removed:
    backup = f"{path}.bak-cua-driver-local-uninstall-{int(time.time())}"
    shutil.copy2(path, backup)
    fd, temporary = tempfile.mkstemp(prefix=".claude.json.", dir=os.path.dirname(path) or ".", text=True)
    with os.fdopen(fd, "w", encoding="utf-8") as handle:
        json.dump(data, handle, indent=2, ensure_ascii=False)
        handle.write("\n")
    os.replace(temporary, path)
    print(f"==> removed local Claude MCP registration(s): {', '.join(removed)}")
PY
fi

# Exact local-only directories. The release app, state, cache and services use
# names without the -local suffix and cannot be reached by these paths.
[[ ! -d "$CACHE_DIR" ]] || { rm -rf "$CACHE_DIR"; log "removed $CACHE_DIR"; }

# A caller may override CUA_DRIVER_LOCAL_HOME, so never recursively delete that
# root. Require the installer's local executable marker, remove only known
# runtime-owned children, and leave any unrelated files in place.
LOCAL_HOME_MARKER="$HOME_DIR/packages/current/cua-driver-local"
if [[ -e "$LOCAL_HOME_MARKER" || -L "$LOCAL_HOME_MARKER" ]]; then
    rm -rf "$HOME_DIR/packages" "$HOME_DIR/skills"
    rm -f \
        "$HOME_DIR/.installation_recorded" \
        "$HOME_DIR/.telemetry_enabled" \
        "$HOME_DIR/.telemetry_id" \
        "$HOME_DIR/.telemetry_install_channel" \
        "$HOME_DIR/.tcc-signing-identity" \
        "$HOME_DIR/config.json" \
        "$HOME_DIR/serve.err.log" \
        "$HOME_DIR/serve.out.log" \
        "$HOME_DIR/version_check.json"
    if rmdir "$HOME_DIR" 2>/dev/null; then
        log "removed empty local home $HOME_DIR"
    else
        log "removed local runtime payloads from $HOME_DIR; preserved unrelated files"
    fi
elif [[ -d "$HOME_DIR" ]]; then
    log "$HOME_DIR has no cua-driver-local install marker; leaving it untouched"
fi
if [[ "$OS" == "Darwin" && -d "$APP_BUNDLE" ]]; then
    if [[ -w "$(dirname "$APP_BUNDLE")" ]]; then
        rm -rf "$APP_BUNDLE"
    else
        sudo rm -rf "$APP_BUNDLE"
    fi
    log "removed $APP_BUNDLE"
fi

log "cua-driver-local uninstalled; release cua-driver was left untouched"
