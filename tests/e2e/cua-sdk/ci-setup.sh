#!/usr/bin/env bash
# CI setup for the cua SDK e2e suite (Linux runners; also fine on a Mac with
# the brief's toolchain). Builds everything the lanes need:
#
#   the cua SDK (release cdylib, staged for Python and Node), the cua CLI and
#   cua-test-fixtures (debug), the TS package and its wasm browser build,
#   Playwright's headless Chromium (into $CUA_E2E_PLAYWRIGHT_DIR), gVisor.
#   --images  linux (with a freshly built cua-spacesd) and the
#             two plain images, rootfs form, host arch
#   --disks   the VM disks of linux and plain/ubuntu-server into
#             ~/.cache/cua-images-e2e, plus QEMU and firmware
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
IMAGES=0; DISKS=0
for a in "$@"; do
    case "$a" in --images) IMAGES=1 ;; --disks) DISKS=1 ;; *) echo "unknown $a" >&2; exit 2 ;; esac
done
arch="$(uname -m)"; case "$arch" in arm64|aarch64) harch=arm64 ;; *) harch=amd64 ;; esac
step() { echo "::group::$*"; }
end() { echo "::endgroup::"; }

if [ "$(uname -s)" = Linux ]; then
    step "system packages, gVisor"
    sudo apt-get update -qq
    pkgs="genisoimage"
    [ "$DISKS" = 1 ] && pkgs="$pkgs qemu-utils qemu-system-x86 qemu-system-arm ovmf qemu-efi-aarch64"
    # shellcheck disable=SC2086
    sudo apt-get install -y -qq --no-install-recommends $pkgs
    [ -e /dev/kvm ] && sudo chmod 666 /dev/kvm || true
    "$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)/scripts/images/install-gvisor.sh"
    docker info --format '{{json .Runtimes}}'
    end
fi

step "cua SDK, CLI, fixtures"
cd "$REPO/libs/cua"
cargo build --locked -p cua-cli -p cua-sdk
scripts/build-test-fixtures.sh
cargo build --locked --release -p cua-sdk
end

step "TypeScript package + browser (wasm) build"
cd "$REPO/libs/cua/typescript"
npm ci
node ../scripts/stage-uniffi-library.mjs --only=python,node
npm run build
if ! command -v wasm-pack >/dev/null; then
    curl -fsSL https://rustwasm.github.io/wasm-pack/installer/init.sh | sh -s -- -f
fi
command -v wasm-bindgen >/dev/null || cargo install --locked wasm-bindgen-cli --version 0.2.126
npm run build:browser
end

step "Playwright Chromium"
pw="${CUA_E2E_PLAYWRIGHT_DIR:-$REPO/tests/e2e/cua-sdk/.playwright}"
mkdir -p "$pw" && cd "$pw"
[ -f package.json ] || echo '{"type":"module"}' >package.json
npm i -s playwright@1
if [ "$(uname -s)" = Linux ]; then npx playwright install --with-deps chromium-headless-shell; else npx playwright install chromium-headless-shell; fi
end

if [ "$IMAGES" = 1 ]; then
    step "reference images (rootfs, $harch)"
    cd "$REPO/libs/images"
    linux/build-spacesd-linux.sh "$harch"
    ./build.sh linux
    ./build.sh plain/ubuntu-server
    ./build.sh plain/ubuntu-xfce-vnc
    end
fi

if [ "$DISKS" = 1 ]; then
    step "reference disks ($harch) -> ~/.cache/cua-images-e2e"
    cd "$REPO/libs/images"
    [ -x "linux/dist/$harch/cua-spacesd" ] || linux/build-spacesd-linux.sh "$harch"
    ./build.sh linux --tag e2e --outputs rootfs,disk --out "$HOME/.cache/cua-images-e2e"
    ./build.sh plain/ubuntu-server --tag e2e --outputs rootfs,disk --out "$HOME/.cache/cua-images-e2e"
    end
fi
echo "cua e2e setup done"
