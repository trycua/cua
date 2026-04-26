#!/usr/bin/env bash
# cua-driver installer — download the latest signed + notarized tarball
# from GitHub Releases, move CuaDriver.app to /Applications, and symlink
# the `cua-driver` binary into /usr/local/bin so shell users can invoke
# it without typing the bundle path.
#
# Usage (from README + release body):
#   /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver/scripts/install.sh)"
#
# Override the release tag with $CUA_DRIVER_VERSION:
#   CUA_DRIVER_VERSION=0.1.0 /bin/bash -c "$(curl -fsSL .../install.sh)"
#
# Uninstall:
#   /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver/scripts/uninstall.sh)"
set -euo pipefail

REPO="trycua/cua"
APP_NAME="CuaDriver.app"
BINARY_NAME="cua-driver"
TAG_PREFIX="cua-driver-v"
APP_DEST="/Applications/$APP_NAME"
BIN_LINK="/usr/local/bin/$BINARY_NAME"
TMP_DIR=$(mktemp -d)
trap 'rm -rf "$TMP_DIR"' EXIT

log() { printf '==> %s\n' "$*"; }
err() { printf 'error: %s\n' "$*" >&2; }

# --- Sanity checks ------------------------------------------------------

if [[ "$(uname -s)" != "Darwin" ]]; then
    err "cua-driver is macOS-only; uname reports $(uname -s)"
    exit 1
fi

for cmd in curl tar; do
    if ! command -v "$cmd" >/dev/null 2>&1; then
        err "$cmd not found on PATH"
        exit 1
    fi
done

# --- Resolve release tag ------------------------------------------------

if [[ -n "${CUA_DRIVER_VERSION:-}" ]]; then
    TAG="${TAG_PREFIX}${CUA_DRIVER_VERSION#v}"
    log "using version from CUA_DRIVER_VERSION: $TAG"
else
    log "resolving latest $TAG_PREFIX* release via GitHub API"
    TAG=$(curl -fsSL "https://api.github.com/repos/$REPO/releases?per_page=40" \
        | grep -Eo '"tag_name":[[:space:]]*"'"${TAG_PREFIX}"'[^"]+"' \
        | head -n 1 \
        | sed -E 's/.*"'"${TAG_PREFIX}"'([^"]+)"/'"${TAG_PREFIX}"'\1/')
    if [[ -z "$TAG" ]]; then
        err "no release matching ${TAG_PREFIX}* found on $REPO"
        exit 1
    fi
    log "latest release: $TAG"
fi

# --- Download tarball ---------------------------------------------------

ARCH=$(uname -m)
VERSION="${TAG#${TAG_PREFIX}}"
TARBALL="cua-driver-${VERSION}-darwin-${ARCH}.tar.gz"
URL="https://github.com/$REPO/releases/download/$TAG/$TARBALL"

log "downloading $URL"
if ! curl -fsSL -o "$TMP_DIR/$TARBALL" "$URL"; then
    err "download failed; try CUA_DRIVER_VERSION=<version> to pin a specific release"
    exit 1
fi

log "extracting"
tar -xzf "$TMP_DIR/$TARBALL" -C "$TMP_DIR"

if [[ ! -d "$TMP_DIR/$APP_NAME" ]]; then
    err "$APP_NAME not found inside $TARBALL (tarball layout may have changed)"
    exit 1
fi

# --- Install .app bundle ------------------------------------------------

if [[ -e "$APP_DEST" ]]; then
    log "removing existing $APP_DEST"
    rm -rf "$APP_DEST"
fi

log "installing $APP_DEST"
ditto "$TMP_DIR/$APP_NAME" "$APP_DEST"

# --- Symlink CLI --------------------------------------------------------

APP_BINARY="$APP_DEST/Contents/MacOS/$BINARY_NAME"
if [[ ! -x "$APP_BINARY" ]]; then
    err "binary missing at $APP_BINARY (refusing to create broken symlink)"
    exit 1
fi

SUDO=""
if [[ ! -w "$(dirname "$BIN_LINK")" ]]; then
    SUDO="sudo"
    log "/usr/local/bin requires elevated write; prompting for sudo"
fi

$SUDO mkdir -p "$(dirname "$BIN_LINK")"
$SUDO ln -sf "$APP_BINARY" "$BIN_LINK"
log "symlinked $BIN_LINK -> $APP_BINARY"

# --- Install Claude Code skill pack -------------------------------------
#
# Detect Claude Code users (via ~/.claude/skills/ presence) and drop a
# symlink pointing at the skill we shipped inside the bundle. Auto-updates
# atomically replace /Applications/CuaDriver.app, so the symlink stays
# valid across every release. Never overwrites an existing link or
# directory — dev users with their own ~/.claude/skills/cua-driver symlink
# pointing at a working copy of the repo keep theirs.

SKILL_LINK="$HOME/.claude/skills/cua-driver"
SKILL_TARGET="$APP_DEST/Contents/Resources/Skills/cua-driver"
if [[ -d "$HOME/.claude/skills" ]]; then
    if [[ -e "$SKILL_LINK" ]] || [[ -L "$SKILL_LINK" ]]; then
        log "skill link already exists at $SKILL_LINK (skipping)"
    elif [[ -d "$SKILL_TARGET" ]]; then
        ln -s "$SKILL_TARGET" "$SKILL_LINK"
        log "symlinked Claude Code skill at $SKILL_LINK"
    else
        log "skill pack missing at $SKILL_TARGET (skipping; older release?)"
    fi
fi

# --- Done ---------------------------------------------------------------

log "cua-driver $VERSION installed"
cat << 'FINALEOF'

Next steps:
  1. Start the daemon so TCC attributes requests to CuaDriver.app:
       open -n -g -a CuaDriver --args serve

  2. Trigger the Accessibility + Screen Recording prompts:
       cua-driver check_permissions
     macOS will raise the system dialogs. Grant both, then re-run
     the command to confirm it reports all green.

  3. Wire into your MCP client (Claude Code, Cursor, etc.):
       cua-driver mcp-config | pbcopy

  4. Or drive directly from the shell:
       cua-driver list_apps

Docs: https://github.com/trycua/cua/tree/main/libs/cua-driver
FINALEOF
