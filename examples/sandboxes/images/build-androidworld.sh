#!/usr/bin/env bash
# Build and push the ghcr.io/<org>/androidworld:latest OCI artifact.
#
# Pre-bakes an Android AVD (Android Virtual Device) with all 20 AndroidWorld
# apps installed, then pushes it as a multi-arch OCI artifact to GHCR so that:
#   Image.from_registry("ghcr.io/<org>/androidworld:latest")
# boots natively with HVF (Apple Silicon) or KVM/HAXM (x86_64) — no Docker.
#
# Prerequisites
# -------------
#   - Android SDK installed to ~/.cua/android-sdk  (auto-installed if missing)
#   - Java (brew install openjdk on macOS)
#   - android_world source at /tmp/android_world
#       git clone https://github.com/google-research/android_world /tmp/android_world
#   - A Python venv with android_world deps:
#       uv venv /tmp/aw-venv --python 3.12
#       uv pip install --python /tmp/aw-venv/bin/python 'setuptools<74' grpcio-tools==1.71.0 protobuf==5.29.5
#       /tmp/aw-venv/bin/python -c "
#         import sys, os; sys.path.insert(0, '/tmp/android_world')
#         from grpc_tools import protoc; import pkg_resources
#         root = '/tmp/android_world'
#         grpc_inc = pkg_resources.resource_filename('grpc_tools', '_proto')
#         for proto in ['android_world/task_evals/information_retrieval/proto/state.proto',
#                       'android_world/task_evals/information_retrieval/proto/task.proto']:
#             protoc.main(['grpc_tools.protoc', f'--proto_path={grpc_inc}', f'--proto_path={root}',
#                          f'--python_out={root}', f'--grpc_python_out={root}', os.path.join(root, proto)])
#       "
#       uv pip install --python /tmp/aw-venv/bin/python \
#           absl-py dm-env grpcio==1.71.0 lxml Pillow numpy opencv-python-headless fastapi uvicorn httpx
#   - GITHUB_TOKEN with write:packages scope  (export GITHUB_TOKEN=$(gh auth token))
#   - GITHUB_USERNAME set to your GitHub username
#
# Usage
# -----
#   export GITHUB_TOKEN=$(gh auth token)
#   export GITHUB_USERNAME=<your-github-username>
#   bash build-androidworld.sh
#
#   # Push only (AVD already baked at ~/.cua/android-avd/androidworld.avd):
#   bash build-androidworld.sh --push-only
#
#   # Bake only (no push):
#   bash build-androidworld.sh --bake-only
#
#   # Specify arch (default: auto-detect from host):
#   bash build-androidworld.sh --arch amd64
#
# Multi-arch
# ----------
# Run on Apple Silicon for arm64, then on x86_64 Linux with --arch amd64.
# Both pushes update the same manifest index at :latest.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"

: "${GITHUB_USERNAME:?Set GITHUB_USERNAME to your GitHub username}"
: "${GITHUB_TOKEN:?Set GITHUB_TOKEN (gh auth token) before pushing}"

ORG="${REGISTRY_ORG:-trycua}"
REF="ghcr.io/${ORG}/androidworld:latest"
AVD_NAME="androidworld"
AVD_HOME="${HOME}/.cua/android-avd"
AVD_DIR="${AVD_HOME}/${AVD_NAME}.avd"
AW_SOURCE_DIR="${AW_SOURCE_DIR:-/tmp/android_world}"
AW_VENV="${AW_VENV:-/tmp/aw-venv}"
ARCH="${ARCH:-}"
BAKE_ONLY=false
PUSH_ONLY=false

for arg in "$@"; do
  case $arg in
    --bake-only) BAKE_ONLY=true ;;
    --push-only) PUSH_ONLY=true ;;
    --arch=*) ARCH="${arg#--arch=}" ;;
    --arch) shift; ARCH="${1:-}" ;;
  esac
done

# ── Step 1: Bake ─────────────────────────────────────────────────────────────
if [ "$PUSH_ONLY" != "true" ]; then
  echo "==> Baking AndroidWorld AVD (~15-30 min) ..."
  AW_SOURCE_DIR="$AW_SOURCE_DIR" AW_VENV="$AW_VENV" \
    "$AW_VENV/bin/python" "${SCRIPT_DIR}/bake_androidworld_avd.py"
  echo "==> Bake complete: ${AVD_DIR}"
fi

# ── Step 2: Push ─────────────────────────────────────────────────────────────
if [ "$BAKE_ONLY" != "true" ]; then
  echo "==> Pushing ${AVD_DIR} → ${REF}${ARCH:+ (arch=${ARCH})} ..."

  PUSH_ARGS=(--avd-dir "$AVD_DIR" --ref "$REF" --agent-type androidworld)
  [ -n "$ARCH" ] && PUSH_ARGS+=(--arch "$ARCH")

  source "${REPO_ROOT}/libs/python/cua-sandbox/.venv/bin/activate"

  ORAS_USERNAME="$GITHUB_USERNAME" ORAS_PASSWORD="$GITHUB_TOKEN" \
    python -m cua_sandbox.registry.avd_builder push "${PUSH_ARGS[@]}"

  echo "==> Done: ${REF}"
fi
