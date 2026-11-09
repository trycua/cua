#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Photograph the running app against a real local Space.
#
#   scripts/capture.sh <out-dir> [<scratch-home>] [--phone]
#
# With --phone, the Mac app stays up after its tour and the iPhone screens
# (../cua-bots-ios, in its macOS preview host) are photographed against the
# same bot, sending messages and approvals from the phone side.
#
# Runs in a throwaway HOME with the file credential store, so it never reads
# or writes your ~/.cua, Keychain or app data. The bots talk to the scripted
# mock provider (`cua-mock-llm`, scripts/demo-model.json) through the real
# harness in a real Space; every reply says what it is. Screenshots use a
# neutral identity (CUA_BOTS_OWNER=Maya, CUA_BOTS_DEVICE_NAME=maya-mbp), not
# your account and Mac names, in light appearance (CUA_BOTS_APPEARANCE, this
# app only). Needs Docker (or
# Colima), the staged Cua Spaces app export, and `cua` and `cua-mock-llm` built:
#   (cd ../../libs/cua && cargo build --locked --release -p cua-spaces-ffi \
#     && cargo build --locked --release -p cua-cli -p cua-mock-llm)
#   ../../libs/spaces-app-swift/scripts/stage-library.sh
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
out=$(mkdir -p "$1" && cd "$1" && pwd)
scratch=${2:-$(mktemp -d "${TMPDIR:-/tmp}/cua-bots-home.XXXXXX")}
phone=${3:-}
mkdir -p "$scratch"
target=${CARGO_TARGET_DIR:-../../libs/cua/target}
cua="$target/release/cua"
mock="$target/release/cua-mock-llm"
port=${CUA_BOTS_MOCK_PORT:-8787}

if [[ -z ${DOCKER_HOST:-} && -S $HOME/.colima/default/docker.sock ]]; then
  export DOCKER_HOST="unix://$HOME/.colima/default/docker.sock"
fi

swift build --product CuaBots
if [[ $phone == --phone ]]; then (cd ../cua-bots-ios && swift build --product CuaBotsPhonePreview); fi

"$mock" --listen "127.0.0.1:$port" --scripts scripts/demo-model.json >"$out/mock-llm.log" 2>&1 &
mock_pid=$!

env HOME="$scratch" CUA_HOME="$scratch/.cua" CUA_CREDENTIAL_STORE=file CUA_TELEMETRY=0 \
  CUA_BOTS_CAPTURE="$out" CUA_BOTS_SYSTEM_NOTIFICATIONS=0 \
  CUA_BOTS_APPEARANCE="${CUA_BOTS_APPEARANCE:-light}" \
  CUA_BOTS_OWNER="${CUA_BOTS_OWNER:-Maya}" CUA_BOTS_DEVICE_NAME="${CUA_BOTS_DEVICE_NAME:-maya-mbp}" \
  CUA_BOTS_CAPTURE_STAY=$([[ $phone == --phone ]] && echo 1 || echo 0) \
  CUA_BOTS_MODEL_URL="http://host.docker.internal:$port" CUA_BOTS_MODEL=claude-mock-1 \
  CUA_BOTS_MODEL_KEY_VARS=ANTHROPIC_API_KEY ANTHROPIC_API_KEY=mock-key \
  .build/debug/CuaBots >"$out/app.log" 2>&1 &
app_pid=$!

phone_pid=
cleanup() {
  [[ -n $phone_pid ]] && kill "$phone_pid" 2>/dev/null || true
  kill "$app_pid" 2>/dev/null || true
  kill "$mock_pid" 2>/dev/null || true
  # Delete the Spaces this run created (they live in the scratch HOME's registry).
  for name in $(HOME="$scratch" "$cua" sb ls --local --json --embedded 2>/dev/null \
                | python3 -c 'import json,sys; [print(s["name"]) for s in json.load(sys.stdin) if s["name"].startswith("bot-")]' 2>/dev/null); do
    HOME="$scratch" "$cua" sb rm "local:$name" --embedded -f >/dev/null 2>&1 || true
  done
  # Window state goes to the real preferences (they don't follow HOME).
  # These two domains belong to this sample's executables only.
  defaults delete CuaBots >/dev/null 2>&1 || true
  defaults delete CuaBotsPhonePreview >/dev/null 2>&1 || true
  rm -f "$HOME/Library/Preferences/CuaBots.plist" "$HOME/Library/Preferences/CuaBotsPhonePreview.plist"
}
trap cleanup EXIT

if [[ $phone == --phone ]]; then
  until grep -q "tour finished" "$out/tour.log" 2>/dev/null || ! kill -0 "$app_pid" 2>/dev/null; do sleep 2; done
  env HOME="$scratch" CUA_HOME="$scratch/.cua" CUA_CREDENTIAL_STORE=file CUA_TELEMETRY=0 \
    CUA_BOTS_APPEARANCE="${CUA_BOTS_APPEARANCE:-light}" CUA_BOTS_PREVIEW_SPACE=local:bot-ada CUA_BOTS_PHONE_CAPTURE="$out/phone" \
    ../cua-bots-ios/.build/debug/CuaBotsPhonePreview >"$out/phone.log" 2>&1 &
  phone_pid=$!
  wait "$phone_pid" || true
  phone_pid=
else
  wait "$app_pid" || true
fi
echo "screenshots in $out (log: $out/tour.log)"
