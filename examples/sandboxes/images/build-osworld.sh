#!/usr/bin/env bash
# Build and push the ghcr.io/<org>/osworld:latest OCI artifact.
#
# Downloads the OSWorld Ubuntu disk(s) from HuggingFace, converts if needed,
# and pushes as a multi-arch OCI artifact so that:
#   Image.from_registry("ghcr.io/<org>/osworld:latest")
# auto-selects the OSWorld Flask transport and runs natively accelerated on
# both x86_64 (KVM) and Apple Silicon (HVF).
#
# x86_64 image : Ubuntu.qcow2.zip  — already qcow2, use directly
# aarch64 image: Ubuntu-arm.zip    — VMware format, converted via qemu-img
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
#
#   # Push amd64 (run on any host, uses TCG on Apple Silicon):
#   bash build-osworld.sh --arch amd64
#
#   # Push arm64 (run on Apple Silicon for best conversion speed):
#   bash build-osworld.sh --arch arm64
#
#   # Push both (updates the same manifest index each time):
#   bash build-osworld.sh --arch amd64
#   bash build-osworld.sh --arch arm64
#
#   # Use a pre-downloaded / pre-converted qcow2 (skip download + conversion):
#   OSWORLD_QCOW2=/path/to/disk.qcow2 bash build-osworld.sh --arch arm64

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"

# Locate qemu-img (brew, system, or bundled with Android SDK emulator)
QEMU_IMG="qemu-img"
for _candidate in \
    "$(command -v qemu-img 2>/dev/null)" \
    "/opt/homebrew/bin/qemu-img" \
    "/usr/local/bin/qemu-img" \
    "${HOME}/.cua/android-sdk/emulator/qemu-img"; do
  [ -x "$_candidate" ] && { QEMU_IMG="$_candidate"; break; }
done

: "${GITHUB_USERNAME:?Set GITHUB_USERNAME to your GitHub username}"
: "${GITHUB_TOKEN:?Set GITHUB_TOKEN (gh auth token) before pushing}"

ORG="${REGISTRY_ORG:-trycua}"
REF="ghcr.io/${ORG}/osworld:latest"
CACHE_DIR="${HOME}/.cua/cua-sandbox/image-cache/osworld"

UBUNTU_AMD64_URL="https://huggingface.co/datasets/xlangai/ubuntu_osworld/resolve/main/Ubuntu.qcow2.zip"
UBUNTU_ARM_URL="https://huggingface.co/datasets/xlangai/ubuntu_osworld/resolve/main/Ubuntu-arm.zip"

# ── Parse args ────────────────────────────────────────────────────────────────
ARCH=""
for arg in "$@"; do
  case $arg in
    --arch=*) ARCH="${arg#--arch=}" ;;
    --arch) shift; ARCH="${1:-}" ;;
  esac
done

# Default: auto-detect from host
if [ -z "$ARCH" ]; then
  HOST_MACHINE="$(uname -m)"
  if [ "$HOST_MACHINE" = "arm64" ] || [ "$HOST_MACHINE" = "aarch64" ]; then
    ARCH="arm64"
  else
    ARCH="amd64"
  fi
fi

echo "==> Building OSWorld image for arch=${ARCH}"
mkdir -p "$CACHE_DIR"

# ── Step 1: Obtain qcow2 for the requested arch ───────────────────────────────
QCOW2="${OSWORLD_QCOW2:-}"

if [ "$ARCH" = "amd64" ]; then
  if [ -z "$QCOW2" ]; then
    QCOW2="${CACHE_DIR}/Ubuntu-amd64.qcow2"
    if [ ! -f "$QCOW2" ]; then
      # Accept older cached name from pre-multi-arch builds
      if [ -f "${CACHE_DIR}/Ubuntu.qcow2" ]; then
        QCOW2="${CACHE_DIR}/Ubuntu.qcow2"
      else
        ZIP="${CACHE_DIR}/Ubuntu.qcow2.zip"
        echo "==> Downloading OSWorld x86_64 image (~8 GB) ..."
        curl -L --progress-bar -o "$ZIP" "$UBUNTU_AMD64_URL"
        echo "==> Extracting ..."
        # ZIP64 format — use ditto on macOS, unzip on Linux
        if command -v ditto &>/dev/null; then
          ditto -xk "$ZIP" "$CACHE_DIR"
        else
          unzip -o "$ZIP" -d "$CACHE_DIR"
        fi
        rm -f "$ZIP"
        [ -f "${CACHE_DIR}/Ubuntu.qcow2" ] && mv "${CACHE_DIR}/Ubuntu.qcow2" "$QCOW2"
      fi
    fi
  fi
elif [ "$ARCH" = "arm64" ]; then
  if [ -z "$QCOW2" ]; then
    QCOW2="${CACHE_DIR}/Ubuntu-arm64.qcow2"
    if [ ! -f "$QCOW2" ]; then
      ZIP="${CACHE_DIR}/Ubuntu-arm.zip"
      echo "==> Downloading OSWorld ARM image ..."
      curl -L --progress-bar -o "$ZIP" "$UBUNTU_ARM_URL"
      echo "==> Extracting VMware archive ..."
      EXTRACT_DIR="${CACHE_DIR}/Ubuntu-arm-extracted"
      mkdir -p "$EXTRACT_DIR"
      if command -v ditto &>/dev/null; then
        ditto -xk "$ZIP" "$EXTRACT_DIR"
      else
        unzip -o "$ZIP" -d "$EXTRACT_DIR"
      fi
      rm -f "$ZIP"

      # Find the VMware disk file — .vmdk or possibly already .qcow2
      VMDK=$(find "$EXTRACT_DIR" -name "*.vmdk" | head -1)
      RAW_QCOW2=$(find "$EXTRACT_DIR" -name "*.qcow2" | head -1)

      if [ -n "$RAW_QCOW2" ]; then
        echo "==> Found qcow2 directly: ${RAW_QCOW2}"
        mv "$RAW_QCOW2" "$QCOW2"
        rm -rf "$EXTRACT_DIR"
      elif [ -n "$VMDK" ]; then
        echo "==> Converting VMware disk to qcow2: $(basename "$VMDK") → Ubuntu-arm64.qcow2 ..."
        "$QEMU_IMG" convert -p -f vmdk -O qcow2 "$VMDK" "$QCOW2"
        rm -rf "$EXTRACT_DIR"
      else
        echo "ERROR: Could not find a .vmdk or .qcow2 inside ${EXTRACT_DIR}"
        ls -la "$EXTRACT_DIR"
        exit 1
      fi
    fi
  fi
else
  echo "ERROR: Unknown arch '${ARCH}'. Use --arch amd64 or --arch arm64."
  exit 1
fi

echo "==> Source: ${QCOW2} ($(du -sh "$QCOW2" | cut -f1))"

# ── Step 2: Push to GHCR ─────────────────────────────────────────────────────
echo "==> Pushing → ${REF} (arch=${ARCH}) ..."

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
    --disk-size-gb 64 \
    --arch "$ARCH"

echo "==> Done: ${REF} (arch=${ARCH})"
