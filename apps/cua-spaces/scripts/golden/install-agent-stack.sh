#!/bin/bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# install-agent-stack.sh — install the services that let an agent see and drive
# this Space, and that receive teleported sessions and files.
#
# Run INSIDE the guest with the prebuilt artifacts staged in $STAGE
# (default ~/.cua/stage):
#
#   CuaDriverLocal.app/     the cua-driver daemon, as a SIGNED APP BUNDLE
#   Cua Spacesd.app/     cua-spacesd, as a SIGNED APP BUNDLE
#                           (bundle id com.trycua.cua-env-driver; build it with
#                           libs/cua-spacesd/scripts/build-macos-app.sh)
#
# The daemons must be .app bundles, not loose binaries: macOS grants screen
# recording and accessibility to a bundle identity, and the golden's TCC
# database is seeded against those identities. A bare binary gets no grants and
# silently captures nothing.
#
# What each provides:
#   cua-spacesd :3211 THE in-sandbox daemon (gRPC + gRPC-Web on one port):
#                        exec, files, window/desktop streams (media wire v2 on
#                        /media, QUIC on 3212), presence, teleport import
#                        (TeleportService), hotspot/tunnels, and the
#                        cua-driver tools on /mcp. It replaces computer-server,
#                        rcdpd, rcdp-handoff and rcdp-socks.
#   cua-driver-local     the local driver daemon, over a unix socket in
#                        ~/Library/Caches/cua-driver-local. Agents INSIDE the
#                        Space reach it with `cua-driver-local mcp --socket …`
#                        (stdio; see write-agent-mcp.sh), no token needed.
#
# Token: no long-lived secret is baked into the image. When
# ~/.cua/spacesd/token exists (a provisioner wrote one, or the host delivered
# one through the Lume setup share at /Volumes/My Shared Files/env-token) the
# driver uses it;
# otherwise it starts in bootstrap mode (--insecure-bootstrap): only
# GetCapabilities, Health and Init answer until the first client (the cua SDK /
# `cua daemon`) installs a fresh token with SystemService.Init. The token never
# goes on argv.
set -uo pipefail

STAGE="${STAGE:-$HOME/.cua/stage}"
BIN="$HOME/.local/bin"
SRV="$HOME/.cua-server"
AGENTS="$HOME/Library/LaunchAgents"
DRIVER_APP="/Applications/CuaDriverLocal.app"
ENV_APP="/Applications/Cua Spacesd.app"
DRIVER_SOCK="$HOME/Library/Caches/cua-driver-local/cua-driver-local.sock"

say()  { printf '\033[1;36m==> %s\033[0m\n' "$*"; }
fail() { echo "$*" >&2; exit 1; }

mkdir -p "$BIN" "$SRV" "$AGENTS"

say "Installing app bundles"
for pair in "CuaDriverLocal.app|$DRIVER_APP" "Cua Spacesd.app|$ENV_APP"; do
  src="$STAGE/${pair%%|*}"; dst="${pair##*|}"
  [ -d "$src" ] || fail "missing $src — stage the signed .app bundle (see header)"
  rm -rf "$dst"
  cp -R "$src" "$dst" || fail "could not install $dst"
  # A copied bundle is quarantined; launching it would raise an unanswerable
  # Gatekeeper dialog inside a Space.
  xattr -dr com.apple.quarantine "$dst" 2>/dev/null || true
  echo "  $dst"
done

say "Creating the helper venv"
# permission-approver.py (see prepare-space.sh) needs pyobjc's Quartz bindings,
# which /usr/bin/python3 lacks. It lives in its own venv; seed-tcc.sh grants
# this interpreter's real path. (This venv used to host computer-server.)
export PATH="$BIN:$PATH"
if [ ! -x "$SRV/venv/bin/python" ]; then
  "$BIN/uv" venv --python 3.13 "$SRV/venv" >/dev/null 2>&1 \
    || fail "uv venv failed (run blender-setup.sh first for uv)"
fi
"$BIN/uv" pip install --python "$SRV/venv/bin/python" pyobjc-framework-Quartz >/dev/null 2>&1 \
  || fail "could not install pyobjc-framework-Quartz into the helper venv"
"$SRV/venv/bin/python" -c "import Quartz" 2>/dev/null \
  || fail "Quartz not importable in the helper venv"
echo "  $SRV/venv"

say "Writing launchers"

# cua-spacesd runs from its bundle so it keeps its TCC identity. The token
# reaches it through the token file (CUA_ENV_TOKEN_FILE), never argv.
# CUA_ENV_LOGIN_KEYCHAIN_PW: teleported Keychain items land in the LOGIN
# keychain, which sanitize-golden.sh re-keys to the account password ("lume"),
# so the importer can unlock it non-interactively. CUA_ENV_KEYCHAIN is left
# unset on purpose: the default keychain IS login on this image, and Unity Hub's
# keyring only resolves the default keychain.
cat > "$SRV/start_spacesd.sh" <<'EOF'
#!/bin/bash
mkdir -p "$HOME/.cua/spacesd"
export CUA_ENV_TOKEN_FILE="$HOME/.cua/spacesd/token"
export CUA_ENV_LOGIN_KEYCHAIN_PW="lume"
export CUA_ENV_LOG="${CUA_ENV_LOG:-info}"
# A token the host delivered through the Lume setup share (cua-vmm writes
# env-token into a read-only share) replaces any older one. The share mounts
# around login, so wait for it briefly (bounded).
share="/Volumes/My Shared Files"
for _ in $(seq 1 10); do [ -d "$share" ] && break; sleep 0.5; done
if [ -s "$share/env-token" ]; then
  install -m 0600 "$share/env-token" "$CUA_ENV_TOKEN_FILE"
fi
bootstrap=()
[ -s "$CUA_ENV_TOKEN_FILE" ] || bootstrap=(--insecure-bootstrap)
exec "/Applications/Cua Spacesd.app/Contents/MacOS/cua-spacesd" serve \
  --listen 0.0.0.0:3211 "${bootstrap[@]}"
EOF

chmod +x "$SRV"/*.sh

write_agent() {  # label, program-args...
  local label="$1"; shift
  local args=""
  for a in "$@"; do args="${args}    <string>${a}</string>
"; done
  cat > "$AGENTS/${label}.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>${label}</string>
  <key>ProgramArguments</key>
  <array>
${args}  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>${SRV}/${label}.out.log</string>
  <key>StandardErrorPath</key><string>${SRV}/${label}.err.log</string>
</dict>
</plist>
EOF
  echo "  $label"
}

say "Installing LaunchAgents"
write_agent com.trycua.cua-driver-local "$DRIVER_APP/Contents/MacOS/cua-driver-local" serve
write_agent com.trycua.spacesd      /bin/bash "$SRV/start_spacesd.sh"

say "Loading"
# Remove the daemons an older image may still carry (computer-server,
# rcdpd, rcdp-handoff, and cua-spacesd under its older names cua-guestd and
# cua-env-driver) so nothing else binds their ports.
for old in com.trycua.computer_server com.trycua.rcdphost com.trycua.rcdp-handoff \
  com.trycua.guestd com.trycua.env_driver; do
  launchctl bootout "gui/$(id -u)/$old" 2>/dev/null
  rm -f "$AGENTS/$old.plist"
done
rm -rf "/Applications/RCDP Host.app" "/Applications/Cua Guestd.app"
rm -f "$SRV/start_server.sh" "$SRV/start_rcdpd.sh" "$SRV/start_handoff.sh" "$BIN/rcdp-handoff" \
  "$SRV/start_guestd.sh" \
  "$HOME/.rcdp-token" "$HOME/.rcdp-handoff-token" "$HOME/.cua/env-token"
for l in com.trycua.cua-driver-local com.trycua.spacesd; do
  launchctl bootout "gui/$(id -u)/$l" 2>/dev/null
  launchctl bootstrap "gui/$(id -u)" "$AGENTS/$l.plist" 2>/dev/null \
    || echo "  (bootstrap $l deferred to next login)"
done

say "Waiting for cua-spacesd (:3211 /health)"
# Poll rather than sampling once — a single early check reports a false failure.
# /health answers without a token (also in bootstrap mode) with 204.
healthy() { [ "$(curl -s -o /dev/null -m 3 -w '%{http_code}' http://127.0.0.1:3211/health)" = 204 ]; }
ok=0
deadline=$((SECONDS + 60))
while [ "$SECONDS" -lt "$deadline" ]; do
  healthy && { ok=1; break; }
  sleep 3
done
if [ "$ok" = 1 ]; then
  echo "  cua-spacesd healthy on :3211"
else
  echo "  WARNING: cua-spacesd is not answering on :3211/health"
  for l in com.trycua.spacesd com.trycua.cua-driver-local; do
    echo "  --- $l ---"; tail -5 "$SRV/$l.err.log" 2>/dev/null | sed 's/^/    /'
  done
fi
