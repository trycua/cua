#!/usr/bin/env bash
# Build and push the ghcr.io/<org>/osworld:latest OCI artifact.
#
# Downloads the OSWorld Ubuntu qcow2 from HuggingFace, then pushes it as an
# OCI artifact annotated with agent_type=osworld so that:
#   Image.from_registry("ghcr.io/<org>/osworld:latest")
# auto-selects the OSWorld Flask transport without any explicit override.
#
# Prerequisites
# -------------
#   - qemu-img in PATH  (brew install qemu on macOS; apt install qemu-utils on Linux)
#   - GITHUB_TOKEN with write:packages scope  (export GITHUB_TOKEN=$(gh auth token))
#   - GITHUB_USERNAME set to your GitHub username
#
# Usage
# -----
#   export GITHUB_TOKEN=$(gh auth token)
#   export GITHUB_USERNAME=<your-github-username>
#   bash build-osworld.sh
#
#   # Use a pre-downloaded qcow2 (skip the ~8 GB download):
#   OSWORLD_QCOW2=/path/to/Ubuntu.qcow2 bash build-osworld.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"

: "${GITHUB_USERNAME:?Set GITHUB_USERNAME to your GitHub username}"
: "${GITHUB_TOKEN:?Set GITHUB_TOKEN (gh auth token) before pushing}"

ORG="${REGISTRY_ORG:-trycua}"
REF="ghcr.io/${ORG}/osworld:latest"
OSWORLD_URL="https://huggingface.co/datasets/xlangai/ubuntu_osworld/resolve/main/Ubuntu.qcow2.zip"
CACHE_DIR="${HOME}/.cua/cua-sandbox/image-cache/osworld"
QCOW2="${OSWORLD_QCOW2:-${CACHE_DIR}/Ubuntu.qcow2}"

mkdir -p "$CACHE_DIR"

# ── Step 1: Download if not cached ───────────────────────────────────────────
if [ ! -f "$QCOW2" ]; then
  ZIP="${CACHE_DIR}/Ubuntu.qcow2.zip"
  echo "==> Downloading OSWorld image (~8 GB) ..."
  curl -L --progress-bar -o "$ZIP" "$OSWORLD_URL"
  echo "==> Extracting ..."
  unzip -o "$ZIP" -d "$CACHE_DIR"
  rm -f "$ZIP"
  QCOW2="${CACHE_DIR}/Ubuntu.qcow2"
fi

echo "==> Source: ${QCOW2} ($(du -sh "$QCOW2" | cut -f1))"

# ── Step 2: Push to GHCR ─────────────────────────────────────────────────────
echo "==> Pushing → ${REF} ..."

source "${REPO_ROOT}/libs/python/cua-sandbox/.venv/bin/activate"

ORAS_USERNAME="$GITHUB_USERNAME" ORAS_PASSWORD="$GITHUB_TOKEN" \
  python -m cua_sandbox.registry.push \
    --ref "$REF" \
    --source "$QCOW2" \
    --kind vm \
    --agent-type osworld \
    --guest-os linux \
    --cpu 4 \
    --ram-mb 8192 \
    --disk-size-gb 64

echo "==> Done: ${REF}"
