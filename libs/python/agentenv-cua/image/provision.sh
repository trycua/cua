#!/bin/bash
# Provision the AgentEnv macOS bench image. Runs INSIDE a macOS VM cloned from ghcr.io/trycua/macos:26 (autologin
# user `lume`, SIP off, ssh on), as that user; build.sh copies this folder in and runs it. Idempotent.
#
# Installs:
#   - CuaDriver.app 0.38.0 from the cua-driver-rs release (sha256-checked), in /Applications
#   - the cua-driver macOS driver at /opt/agentenv/drivers/cua-driver, with a venv holding the AgentEnv protocol
#   - TCC rows (Accessibility, Screen Recording, PostEvent) for com.trycua.driver, bound to the installed bundle
#   - /opt/agentenv/bin: set-display, a GNU `stat -c` shim and a python3 with a current Tk, first on the PATH of
#     commands the driver runs. The driver's start script runs set-display to put the guest's display in the size
#     lume gives the virtual display (a clone otherwise boots in the mode the image was last in)
#   - LaunchAgents that start the cua-driver daemon and the driver server in the GUI session at login; the driver
#     reads its boot settings from /Volumes/My Shared Files/agentenv/boot.json, which the lume provider shares
#
# Usage: provision.sh <payload dir>   (the dir holding this script and the driver folder)
set -euo pipefail

# A non-login ssh shell has only /usr/bin:/bin: add where the base keeps uv, Homebrew and Node.
export PATH="/opt/homebrew/bin:/usr/local/bin:$HOME/.local/bin:$PATH"

PAYLOAD=${1:?payload dir}
CUA_DRIVER_VERSION=0.38.0
TARBALL=cua-driver-rs-${CUA_DRIVER_VERSION}-darwin-arm64.tar.gz
TARBALL_SHA256=5f71191c082ca07c9dfc13b7c9a6eec60a82818c245a93534cc0eb7911471d72
APP_EXECUTABLE_SHA256=b4d974a9c1c92a700cd7be7ede987aea4a1e6835e37039ce3d628b71bd5f0ca8
BUNDLE_ID=com.trycua.driver
DRIVER_DIR=/opt/agentenv/drivers/cua-driver
PYTHON_VERSION=3.12
PROTOCOL_VERSION=0.1.440
LA=$HOME/Library/LaunchAgents

say() { printf '[provision] %s\n' "$*"; }
as_root() { if sudo -n true 2>/dev/null; then sudo "$@"; else printf '%s\n' "${LUME_SSH_PASSWORD:-lume}" | sudo -S -p '' "$@"; fi; }

[ "$(uname -s)" = Darwin ] || { echo "run this inside the macOS VM" >&2; exit 2; }
[ "$(uname -m)" = arm64 ] || { echo "arm64 only" >&2; exit 2; }
csrutil status | grep -q disabled || { echo "SIP must be off in the VM to seed TCC rows" >&2; exit 2; }

say "CuaDriver.app ${CUA_DRIVER_VERSION}"
work=$(mktemp -d)
curl -fsSL -o "$work/$TARBALL" \
  "https://github.com/trycua/cua/releases/download/cua-driver-rs-v${CUA_DRIVER_VERSION}/${TARBALL}"
echo "${TARBALL_SHA256}  $work/$TARBALL" | shasum -a 256 -c -
tar -xzf "$work/$TARBALL" -C "$work"
app_src="$work/cua-driver-rs-${CUA_DRIVER_VERSION}-darwin-arm64/CuaDriver.app"
echo "${APP_EXECUTABLE_SHA256}  $app_src/Contents/MacOS/cua-driver" | shasum -a 256 -c -
as_root rm -rf /Applications/CuaDriver.app
as_root ditto "$app_src" /Applications/CuaDriver.app
as_root xattr -dr com.apple.quarantine /Applications/CuaDriver.app || true
rm -rf "$work"

say "driver at ${DRIVER_DIR}"
as_root mkdir -p "$DRIVER_DIR"
as_root chown -R "$(id -un):staff" /opt/agentenv
cp "$PAYLOAD"/cua_driver_macos/{driver.py,env.toml,requirements.txt,pin_manifest.py} "$DRIVER_DIR/"
[ -f "$PAYLOAD/cua_driver_macos/mcp.json" ] && cp "$PAYLOAD/cua_driver_macos/mcp.json" "$DRIVER_DIR/"
command -v uv >/dev/null || { echo "uv missing: the base must be ghcr.io/trycua/macos:26 (full tier)" >&2; exit 2; }
uv python install "$PYTHON_VERSION"
uv venv --allow-existing -p "$PYTHON_VERSION" "$DRIVER_DIR/venv"
VIRTUAL_ENV="$DRIVER_DIR/venv" uv pip install "agentenv-framework-protocol[computer]==${PROTOCOL_VERSION}"

say "set-display"
as_root mkdir -p /opt/agentenv/bin
as_root chown "$(id -un):staff" /opt/agentenv/bin
command -v swiftc >/dev/null || { echo "swiftc missing: the base must be ghcr.io/trycua/macos:26 (full tier)" >&2; exit 2; }
swiftc -O "$PAYLOAD/set-display.swift" -o /opt/agentenv/bin/set-display

say "GNU stat -c shim"
# The conformance probe is written for Linux and asks `stat -c %u FILE`; the macOS stat spells that `-f %u`. The
# driver puts this directory first on its commands' PATH.
cat > /opt/agentenv/bin/stat <<'SH'
#!/bin/sh
# `stat -c FORMAT FILE...` as GNU stat takes it, on the macOS stat (format letters %u %U %g %G %s %a %n %F).
if [ "$1" = "-c" ] && [ "$#" -ge 3 ]; then
  fmt=$(printf '%s' "$2" | sed -e 's/%U/%Su/g' -e 's/%G/%Sg/g' -e 's/%s/%z/g' -e 's/%a/%Lp/g' -e 's/%n/%N/g' -e 's/%F/%HT/g')
  shift 2
  exec /usr/bin/stat -f "$fmt" "$@"
fi
exec /usr/bin/stat "$@"
SH
chmod 755 /opt/agentenv/bin/stat

say "python3 with a current Tk"
# The probe's Tk fixture runs `python3`. The system python3's Tk 8.5 draws almost none of its widgets on macOS 26 and
# can't go fullscreen, so commands the driver runs get the driver's own python (a uv build with Tk 9) as python3.
cat > /opt/agentenv/bin/python3 <<SH
#!/bin/sh
exec "$DRIVER_DIR/venv/bin/python" "\$@"
SH
chmod 755 /opt/agentenv/bin/python3

say "no Cua Spacesd"
# The base image starts Cua Spacesd at login, and its window covers the whole screen, over whatever an agent or the
# conformance fixture draws. A bench computer is driven through the AgentEnv driver, so turn it off.
if [ -f /Library/LaunchAgents/com.trycua.spacesd.plist ]; then
  as_root mv /Library/LaunchAgents/com.trycua.spacesd.plist /Library/LaunchAgents/com.trycua.spacesd.plist.disabled
fi
launchctl bootout "gui/$(id -u)/com.trycua.spacesd" 2>/dev/null || true
pkill -x cua-spacesd 2>/dev/null || true

say "no Login Items banners"
# Each clone's first boot shows a "Software from ... can run in the background" alert over the top right of the
# screen, where a click aimed at the screen lands on it (and opens System Settings) instead of on the app under it.
# The agent that posts it is off for this user.
launchctl bootout "gui/$(id -u)/com.apple.backgroundtaskmanagement.agent" 2>/dev/null || true
launchctl disable "gui/$(id -u)/com.apple.backgroundtaskmanagement.agent"

say "TCC rows for ${BUNDLE_ID}"
req=$(codesign -d -r- /Applications/CuaDriver.app 2>&1 | sed -n 's/^\(# *\)\{0,1\}designated => //p')
[ -n "$req" ] || { echo "CuaDriver.app has no designated requirement" >&2; exit 1; }
echo "$req" | csreq -r- -b "$TMPDIR/cuadriver.csreq"
hex=$(xxd -p "$TMPDIR/cuadriver.csreq" | tr -d '\n')
system_db="/Library/Application Support/com.apple.TCC/TCC.db"
for service in kTCCServiceAccessibility kTCCServiceScreenCapture kTCCServicePostEvent; do
  as_root sqlite3 "$system_db" "INSERT OR REPLACE INTO access (service, client, client_type, auth_value, auth_reason,
    auth_version, csreq, policy_id, indirect_object_identifier_type, indirect_object_identifier,
    indirect_object_code_identity, flags, last_modified, pid, pid_version, boot_uuid, last_reminded)
    VALUES ('$service', '$BUNDLE_ID', 0, 2, 2, 1, X'$hex', NULL, 0, 'UNUSED', NULL, 0,
    CAST(strftime('%s','now') AS INTEGER), NULL, NULL, 'UNUSED', CAST(strftime('%s','now') AS INTEGER));"
done
for pid in $(pgrep -x tccd); do as_root kill "$pid" 2>/dev/null || true; done

say "direct screen-capture approval for ${BUNDLE_ID}"
# macOS 15+ asks once per app, even with Screen Recording granted, before the first direct capture. replayd keeps
# the answer in the user's ScreenCaptureApprovals.plist; an entry keyed by the bundle identifier counts as approved
# (it is how a managed Mac records the approval ahead of time), and cua-driver reads it as recorded consent.
approvals="$HOME/Library/Group Containers/group.com.apple.replayd/ScreenCaptureApprovals.plist"
mkdir -p "$(dirname "$approvals")"
defaults write "$approvals" "$BUNDLE_ID" -dict \
  kScreenCaptureApprovalLastAlerted -date "3024-01-01 00:00:00 +0000" \
  kScreenCaptureApprovalLastUsed -date "3024-01-01 00:00:00 +0000"
killall -HUP replayd >/dev/null 2>&1 || true
defaults read "$approvals" "$BUNDLE_ID" | grep -Fq kScreenCaptureApprovalLastAlerted

say "LaunchAgents"
mkdir -p "$LA" "$HOME/Library/Logs/agentenv"
cat > "$DRIVER_DIR/start-driver.sh" <<'SH'
#!/bin/bash
# Start the driver server with the boot settings the lume provider shares (driver and app settings, the control
# token). Waits for the share to mount; a computer with no share starts with none.
set -euo pipefail
boot="/Volumes/My Shared Files/agentenv/boot.json"
for _ in $(seq 1 60); do [ -f "$boot" ] && break; sleep 1; done
# The guest boots in the display mode its disk last had; take the size lume gave the virtual display.
/opt/agentenv/bin/set-display || true
export PATH="/opt/agentenv/bin:$PATH"
cd /opt/agentenv/drivers/cua-driver
if [ -f "$boot" ]; then
  eval "$(venv/bin/python -c 'import json,shlex,sys; e=json.load(open(sys.argv[1]))["env"]; print("\n".join(f"export {k}={shlex.quote(v)}" for k,v in e.items() if k.replace("_","").isalnum()))' "$boot")"
fi
export ENVIRONMENT_NAME="${ENVIRONMENT_NAME:-cua-driver}"
exec venv/bin/python -m agentenv_protocol.computer.serve driver.py:CuaDriverMacOS
SH
chmod 755 "$DRIVER_DIR/start-driver.sh"
write_agent() {  # label, program args...
  local label=$1; shift
  {
    printf '<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n'
    printf '<plist version="1.0"><dict>\n<key>Label</key><string>%s</string>\n<key>ProgramArguments</key><array>\n' "$label"
    for arg in "$@"; do printf '<string>%s</string>\n' "$arg"; done
    printf '</array>\n<key>WorkingDirectory</key><string>%s</string>\n' "${WORKDIR:-$DRIVER_DIR}"
    [ -z "${AGENT_ENV:-}" ] || printf '<key>EnvironmentVariables</key><dict>\n%s</dict>\n' "$AGENT_ENV"
    printf '<key>RunAtLoad</key><true/>\n<key>KeepAlive</key><true/>\n<key>LimitLoadToSessionType</key><string>Aqua</string>\n'
    printf '<key>StandardOutPath</key><string>%s/Library/Logs/agentenv/%s.log</string>\n' "$HOME" "$label"
    printf '<key>StandardErrorPath</key><string>%s/Library/Logs/agentenv/%s.log</string>\n</dict></plist>\n' "$HOME" "$label"
  } > "$LA/$label.plist"
}
# launchd must spawn CuaDriver.app's own executable: macOS attributes a process to whatever launchd spawned, and
# a wrapper (python, a shell) that execs it afterwards would be the "responsible" process, which holds no grants.
mkdir -p "$HOME/Library/Caches/agentenv"
chmod 700 "$HOME/Library/Caches/agentenv"
WORKDIR=$HOME AGENT_ENV='<key>DO_NOT_TRACK</key><string>1</string>
<key>CUA_DRIVER_RS_TELEMETRY_ENABLED</key><string>false</string>
<key>CUA_DRIVER_RS_UPDATE_CHECK</key><string>false</string>
' write_agent com.trycua.agentenv.cua-driver-daemon /Applications/CuaDriver.app/Contents/MacOS/cua-driver serve \
  --socket "$HOME/Library/Caches/agentenv/cua-driver.sock" --dangerously-bypass-approvals --no-overlay
write_agent com.trycua.agentenv.driver "$DRIVER_DIR/start-driver.sh"

say "identity"
as_root mkdir -p /etc/agentenv
as_root tee /etc/agentenv/image.json >/dev/null <<JSON
{"image": "agentenv-macos-bench", "cua_driver": "${CUA_DRIVER_VERSION}", "cua_driver_app_sha256": "${APP_EXECUTABLE_SHA256}",
 "agentenv_protocol": "${PROTOCOL_VERSION}", "python": "${PYTHON_VERSION}", "macos": "$(sw_vers -productVersion)",
 "macos_build": "$(sw_vers -buildVersion)"}
JSON
say "done; reboot, then check: launchctl print gui/$(id -u)/com.trycua.agentenv.driver"
