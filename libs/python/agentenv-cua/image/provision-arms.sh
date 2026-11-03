#!/bin/bash
# Add the other bench arms and the bench's macOS apps to the AgentEnv macOS bench image. Runs INSIDE a macOS VM that
# provision.sh has already provisioned (CuaDriver.app, the cua-driver driver, TCC rows, the LaunchAgents), as that
# user; build-arms.sh copies this folder in and runs it. Idempotent.
#
# Installs, each pinned and sha256-checked:
#   - agent-desktop 1.0.2 (the release binary) and its driver, at /opt/agentenv/drivers/agent-desktop
#   - macos-harness 0.2.1 (PyPI) and its driver, at /opt/agentenv/drivers/macos-harness
#   - with --arms ...,codex-cu: the ChatGPT app 26.930.51102 into /Applications and the Codex Computer Use driver.
#     Not for a public image: Sky needs a ChatGPT login inside the VM, which is a step of the person who builds the
#     image, once (`codex login`, then open the ChatGPT app once so it installs its plugin); no credential is here.
#   - the bench's apps (BenchLab, the sentinel, cdb-s02 to s04) at /opt/agentenv/apps/<name>, each with a venv, its
#     files and runtimes, and its workspace built
#   - TCC rows by path for the binaries those arms run (Accessibility, Screen Recording, PostEvent), the neutral
#     computer_* helper's screencapture included
#   - one start-driver.sh that picks the driver by the ENVIRONMENT_NAME of the boot settings the lume provider shares
#     (cua-driver, agent-desktop, macos-harness or codex-cu) and starts the apps those settings name, replacing the
#     LaunchAgent's program from provision.sh
#
# Usage: provision-arms.sh <payload dir> [--arms agent-desktop,macos-harness[,codex-cu]]
#   <payload dir> holds macos-drivers/, macos-apps/ and install_app.py.
set -euo pipefail

PAYLOAD=${1:?payload dir}
shift
ARMS=agent-desktop,macos-harness
while [ $# -gt 0 ]; do
  case "$1" in
    --arms) ARMS=$2; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
has_arm() { case ",$ARMS," in *",$1,"*) return 0 ;; *) return 1 ;; esac; }

PYTHON_VERSION=3.12
PROTOCOL_VERSION=0.1.440
AD_VERSION=1.0.2
AD_ARCHIVE=agent-desktop-v${AD_VERSION}-aarch64-apple-darwin.tar.gz
AD_ARCHIVE_SHA256=7bee385196d7b478de80cf05ca4304aaa38f0eba593b3406ad3c247c1773be19
AD_EXECUTABLE_SHA256=bf0a3c9256512f2f57288d37ccd17d7f03665fccc53c2f74011a49b98e61ba59
AD_HELPER_SHA256=522829116cb006b5770c6a82b361f466a4775f794d6af4083e75231a2d6bb551
GPT_VERSION=26.930.51102
GPT_ZIP=ChatGPT-darwin-arm64-${GPT_VERSION}.zip
GPT_ZIP_SHA256=7001df293fd478b2a4e2d8b5f4ea3aae43a2e21d168960256b2f31ac4ee43f6f
GPT_EXECUTABLE_SHA256=1f2989ac3d702e48c740fe4c54b1b6279f9e48e0e4c0ead7d4501464e2ea976e
DRIVERS=/opt/agentenv/drivers
APPS=/opt/agentenv/apps
TOOLS=/opt/agentenv/tools
#: name:Class pairs of the bench's apps.
APP_CLASSES="benchlab:BenchLab bench-sentinel:BenchSentinel cdb-s02:CdbS02 cdb-s03:CdbS03 cdb-s04:CdbS04"

say() { printf '[provision-arms] %s\n' "$*"; }
as_root() { if sudo -n true 2>/dev/null; then sudo "$@"; else printf '%s\n' "${LUME_SSH_PASSWORD:-lume}" | sudo -S -p '' "$@"; fi; }

[ "$(uname -s)" = Darwin ] && [ "$(uname -m)" = arm64 ] || { echo "run this inside the arm64 macOS VM" >&2; exit 2; }
[ -d "$DRIVERS/cua-driver" ] || { echo "run provision.sh first" >&2; exit 2; }
command -v uv >/dev/null || { echo "uv missing" >&2; exit 2; }
as_root mkdir -p "$DRIVERS" "$APPS" "$TOOLS"
as_root chown -R "$(id -un):staff" /opt/agentenv

driver_venv() {  # folder name, requirements.txt in it
  uv python install "$PYTHON_VERSION"
  uv venv --allow-existing -p "$PYTHON_VERSION" "$1/venv"
  VIRTUAL_ENV="$1/venv" uv pip install -r "$1/requirements.txt"
}
install_driver() {  # name, files...
  local name=$1 dir=$DRIVERS/$1
  say "driver $name"
  mkdir -p "$dir"
  cp "$PAYLOAD/macos-drivers/envs/$name"/* "$dir/"
  driver_venv "$dir"
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
tcc_paths=(/usr/sbin/screencapture)

if has_arm agent-desktop; then
  say "agent-desktop ${AD_VERSION}"
  curl -fsSL -o "$work/$AD_ARCHIVE" "https://github.com/vercel-labs/agent-desktop/releases/download/v${AD_VERSION}/${AD_ARCHIVE}"
  echo "${AD_ARCHIVE_SHA256}  $work/$AD_ARCHIVE" | shasum -a 256 -c -
  mkdir -p "$TOOLS/agent-desktop"
  tar -xzf "$work/$AD_ARCHIVE" -C "$TOOLS/agent-desktop"
  echo "${AD_EXECUTABLE_SHA256}  $TOOLS/agent-desktop/agent-desktop" | shasum -a 256 -c -
  echo "${AD_HELPER_SHA256}  $TOOLS/agent-desktop/agent-desktop-macos-helper" | shasum -a 256 -c -
  install_driver agent-desktop
  tcc_paths+=("$TOOLS/agent-desktop/agent-desktop" "$TOOLS/agent-desktop/agent-desktop-macos-helper" "$(python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$DRIVERS/agent-desktop/venv/bin/python")")
fi

if has_arm macos-harness; then
  install_driver macos-harness
  tcc_paths+=("$(python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$DRIVERS/macos-harness/venv/bin/python")")
fi

if has_arm codex-cu; then
  say "ChatGPT ${GPT_VERSION} (the Codex Computer Use arm: needs a ChatGPT login in this VM afterwards)"
  curl -fsSL -o "$work/$GPT_ZIP" "https://persistent.oaistatic.com/codex-app-prod/${GPT_ZIP}"
  echo "${GPT_ZIP_SHA256}  $work/$GPT_ZIP" | shasum -a 256 -c -
  as_root rm -rf /Applications/ChatGPT.app
  as_root ditto -x -k "$work/$GPT_ZIP" /Applications
  echo "${GPT_EXECUTABLE_SHA256}  /Applications/ChatGPT.app/Contents/MacOS/ChatGPT" | shasum -a 256 -c -
  install_driver codex-cu
  tcc_paths+=("$(python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$DRIVERS/codex-cu/venv/bin/python")")
  say "now, as this user: /Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex login"
fi

say "TCC rows by path"
system_db="/Library/Application Support/com.apple.TCC/TCC.db"
for path in "${tcc_paths[@]}"; do
  for service in kTCCServiceAccessibility kTCCServiceScreenCapture kTCCServicePostEvent; do
    as_root sqlite3 "$system_db" "INSERT OR REPLACE INTO access (service, client, client_type, auth_value, auth_reason,
      auth_version, csreq, policy_id, indirect_object_identifier_type, indirect_object_identifier,
      indirect_object_code_identity, flags, last_modified, pid, pid_version, boot_uuid, last_reminded)
      VALUES ('$service', '$path', 1, 2, 4, 1, NULL, NULL, 0, 'UNUSED', NULL, 0,
      CAST(strftime('%s','now') AS INTEGER), NULL, NULL, 'UNUSED', CAST(strftime('%s','now') AS INTEGER));"
  done
done
for pid in $(pgrep -x tccd); do as_root kill "$pid" 2>/dev/null || true; done

say "the bench's apps"
for entry in $APP_CLASSES; do
  name=${entry%%:*} cls=${entry##*:} dir=$APPS/$name
  say "app $name"
  mkdir -p "$dir"
  cp -R "$PAYLOAD/macos-apps/envs/$name"/. "$dir/"
  uv venv --allow-existing -p "$PYTHON_VERSION" "$dir/venv"
  VIRTUAL_ENV="$dir/venv" uv pip install "agentenv-framework-protocol[app]==${PROTOCOL_VERSION}" pydantic
  (cd "$dir" && "$dir/venv/bin/python" "$PAYLOAD/install_app.py" "$dir" "$cls" --sudo)
done
mkdir -p "$APPS/_artifacts"
cp -R "$PAYLOAD/macos-apps/artifacts"/. "$APPS/_artifacts/"

say "start-driver.sh"
cat > "$DRIVERS/start-driver.sh" <<'SH'
#!/bin/bash
# Start the driver the boot settings name, and the apps they name. The lume provider shares the settings as
# /Volumes/My Shared Files/agentenv/boot.json: env.ENVIRONMENT_NAME is the driver's env name; env.AGENTENV_APP_SETTINGS
# is the JSON of each app's settings by its env name, whose keys are the apps this computer has.
set -euo pipefail
boot="/Volumes/My Shared Files/agentenv/boot.json"
for _ in $(seq 1 60); do [ -f "$boot" ] && break; sleep 1; done
PY=/opt/agentenv/drivers/cua-driver/venv/bin/python
if [ -f "$boot" ]; then
  eval "$($PY -c 'import json,shlex,sys; e=json.load(open(sys.argv[1]))["env"]; print("\n".join(f"export {k}={shlex.quote(v)}" for k,v in e.items() if k.replace("_","").isalnum()))' "$boot")"
fi
name="${ENVIRONMENT_NAME:-cua-driver}"
case "$name" in
  cua-driver) dir=cua-driver cls=CuaDriverMacOS ;;
  agent-desktop) dir=agent-desktop cls=AgentDesktopMacOS ;;
  macos-harness) dir=macos-harness cls=MacosHarness ;;
  codex-cu) dir=codex-cu cls=CodexComputerUse ;;
  *) echo "no driver named $name" >&2; exit 2 ;;
esac
for app in $($PY -c 'import json,os; print(" ".join(json.loads(os.environ.get("AGENTENV_APP_SETTINGS") or "{}")))'); do
  adir=/opt/agentenv/apps/$app
  [ -d "$adir" ] || { echo "no app named $app" >&2; continue; }
  case "$app" in benchlab) acls=BenchLab ;; bench-sentinel) acls=BenchSentinel ;; cdb-s0*) acls=CdbS0${app#cdb-s0} ;; *) continue ;; esac
  (cd "$adir" && ENVIRONMENT_NAME="$app" nohup "$adir/venv/bin/python" -m agentenv_protocol.computer.serve_app "app.py:$acls" >> "$HOME/Library/Logs/agentenv/app-$app.log" 2>&1 &)
done
cd "/opt/agentenv/drivers/$dir"
export ENVIRONMENT_NAME="$name"
exec venv/bin/python -m agentenv_protocol.computer.serve "driver.py:$cls"
SH
chmod 755 "$DRIVERS/start-driver.sh"
# The cua-driver daemon starts only for that driver, from its own LaunchAgent; the driver LaunchAgent now runs this.
sed -i '' "s#<string>$DRIVERS/cua-driver/start-driver.sh</string>#<string>$DRIVERS/start-driver.sh</string>#" "$HOME/Library/LaunchAgents/com.trycua.agentenv.driver.plist"

say "identity"
as_root tee -a /etc/agentenv/image.json >/dev/null <<JSON
{"arms": "${ARMS}", "agent_desktop": "${AD_VERSION}", "macos_harness": "0.2.1", "chatgpt_app": "$(has_arm codex-cu && echo "$GPT_VERSION" || echo none)"}
JSON
say "done; reboot, then check: launchctl print gui/$(id -u)/com.trycua.agentenv.driver"
