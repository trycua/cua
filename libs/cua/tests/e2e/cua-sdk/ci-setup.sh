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
#
# With no --only, every phase runs, in the order below (the nightly job, the
# compat workflows and a laptop). The PR workflow runs the phases on separate
# runners and ships their outputs to the test shards:
#
#   --only PHASE[,PHASE...]
#     system        apt packages, gVisor, docker runtimes
#     cli           debug cua CLI + cua-sdk
#     fixtures      cua-test-fixtures (debug)
#     sdk           release cua-sdk
#     stage         npm ci, stage the release library for Python and Node
#     web           TypeScript build, wasm tools, wasm browser build
#     playwright    Playwright's headless Chromium
#     images        the reference images (needs --images)
#     disks         the VM disks (needs --disks)
#     rust-tests    build the Rust half's test binaries into
#                   libs/cua/tests/e2e/cua-sdk/rust/prebuilt (+ manifest.json), for
#                   `run.py --rust-prebuilt`
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../../../.." && pwd)"
IMAGES=0; DISKS=0; ONLY=""
while [ $# -gt 0 ]; do
    case "$1" in
        --images) IMAGES=1 ;;
        --disks) DISKS=1 ;;
        --only) ONLY="${2:?--only needs a phase list}"; shift ;;
        *) echo "unknown $1" >&2; exit 2 ;;
    esac
    shift
done
arch="$(uname -m)"; case "$arch" in arm64|aarch64) harch=arm64 ;; *) harch=amd64 ;; esac
step() { echo "::group::$*"; }
end() { echo "::endgroup::"; }
# run PHASE: true when this invocation includes the phase.
run() {
    [ -z "$ONLY" ] && return 0
    case ",$ONLY," in *",$1,"*) return 0 ;; *) return 1 ;; esac
}
if [ -n "$ONLY" ]; then
    for p in ${ONLY//,/ }; do
        case "$p" in
            system|cli|fixtures|sdk|stage|web|playwright|images|disks|rust-tests) ;;
            *) echo "unknown phase $p" >&2; exit 2 ;;
        esac
    done
    # `images` and `disks` also need their flag: --only images without --images is a mistake.
    run images && [ "$IMAGES" != 1 ] && { echo "--only images needs --images" >&2; exit 2; }
    run disks && [ "$DISKS" != 1 ] && { echo "--only disks needs --disks" >&2; exit 2; }
fi
# Default (no --only): the phases the flags ask for, as before.
phase() {
    if [ -z "$ONLY" ]; then
        case "$1" in
            images) [ "$IMAGES" = 1 ] ;;
            disks) [ "$DISKS" = 1 ] ;;
            rust-tests) return 1 ;;
            *) return 0 ;;
        esac
    else
        run "$1"
    fi
}

if phase system && [ "$(uname -s)" = Linux ]; then
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

if phase cli || phase fixtures || phase sdk; then
    step "cua SDK, CLI, fixtures"
    cd "$REPO/libs/cua"
    if phase cli; then cargo build --locked -p cua-cli -p cua-sdk; fi
    if phase fixtures; then scripts/build-test-fixtures.sh; fi
    if phase sdk; then cargo build --locked --release -p cua-sdk; fi
    end
fi

# npm ci once per checkout: `stage` and `web` each need node_modules.
npm_ci() {
    cd "$REPO/libs/cua/typescript"
    [ -e node_modules/.package-lock.json ] || npm ci
}

if phase stage; then
    step "npm ci, stage the release library"
    npm_ci
    node ../scripts/stage-uniffi-library.mjs --only=python,node
    end
fi

if phase web; then
    step "TypeScript package + browser (wasm) build"
    npm_ci
    npm run build
    if ! command -v wasm-pack >/dev/null; then
        curl -fsSL https://rustwasm.github.io/wasm-pack/installer/init.sh | sh -s -- -f
    fi
    command -v wasm-bindgen >/dev/null || cargo install --locked wasm-bindgen-cli --version 0.2.126
    npm run build:browser
    end
fi

if phase playwright; then
    step "Playwright Chromium"
    pw="${CUA_E2E_PLAYWRIGHT_DIR:-$REPO/libs/cua/tests/e2e/cua-sdk/.playwright}"
    mkdir -p "$pw" && cd "$pw"
    [ -f package.json ] || echo '{"type":"module"}' >package.json
    npm i -s playwright@1
    if [ "$(uname -s)" = Linux ]; then npx playwright install --with-deps chromium-headless-shell; else npx playwright install chromium-headless-shell; fi
    end
fi

if phase rust-tests; then
    step "Rust test binaries"
    cd "$REPO/libs/cua/tests/e2e/cua-sdk/rust"
    out="$REPO/libs/cua/tests/e2e/cua-sdk/rust/prebuilt"
    rm -rf "$out" && mkdir -p "$out"
    # The same build `cargo test` does, minus running it: every test target of
    # the crate, copied next to a manifest that run.py --rust-prebuilt reads.
    cargo test --no-run --message-format=json >"$out/build.json"
    python3 - "$out" <<'PY'
import json, shutil, sys
from pathlib import Path

out = Path(sys.argv[1])
manifest = []
for line in (out / "build.json").read_text().splitlines():
    try:
        msg = json.loads(line)
    except ValueError:
        continue
    exe = msg.get("executable")
    if msg.get("reason") == "compiler-artifact" and exe and msg.get("profile", {}).get("test"):
        name = msg["target"]["name"]
        shutil.copy2(exe, out / name)
        manifest.append({"name": name, "file": name})
if not manifest:
    raise SystemExit("cargo test --no-run produced no test binaries")
(out / "manifest.json").write_text(json.dumps(sorted(manifest, key=lambda m: m["name"]), indent=1) + "\n")
(out / "build.json").unlink()
print("prebuilt:", ", ".join(m["name"] for m in manifest))
PY
    end
fi

if phase images; then
    step "reference images (rootfs, $harch)"
    cd "$REPO/libs/images"
    linux/build-spacesd-linux.sh "$harch"
    ./build.sh linux
    ./build.sh plain/ubuntu-server
    ./build.sh plain/ubuntu-xfce-vnc
    end
fi

if phase disks; then
    step "reference disks ($harch) -> ~/.cache/cua-images-e2e"
    cd "$REPO/libs/images"
    [ -x "linux/dist/$harch/cua-spacesd" ] || linux/build-spacesd-linux.sh "$harch"
    ./build.sh linux --tag e2e --outputs rootfs,disk --out "$HOME/.cache/cua-images-e2e"
    ./build.sh plain/ubuntu-server --tag e2e --outputs rootfs,disk --out "$HOME/.cache/cua-images-e2e"
    end
fi
echo "cua e2e setup done"
