#!/usr/bin/env bash
# The cua-sandbox action's steps, over the `cua` CLI. The action calls one
# phase per step; run it locally with the same environment to reproduce a CI
# run exactly (`all` runs every phase and always cleans up):
#
#   CUA_SB_IMAGE=ghcr.io/trycua/linux:24.04 \
#   CUA_SB_OVERLAY='cua-spacesd=./dist/cua-spacesd' \
#   CUA_SB_RUN='cua sb exec "$CUA_SANDBOX" uname -a' \
#   .github/actions/cua-sandbox/cua-sandbox.sh all
#
# Phases: setup, create, guest-setup, doctor, run, guest-run, collect,
# cleanup, all.
#
# Inputs (environment; the action maps its inputs onto these):
#   CUA_SB_IMAGE          image: canonical (linux, ghcr.io/trycua/linux:24.04),
#                         bench or any registry ref (required)
#   CUA_SB_ON             local (default), docker, qemu, cloud
#   CUA_SB_NAME           sandbox name (default cua-ci-<run>-<attempt>-<job>)
#   CUA_SB_OVERLAY        NAME=PATH[:GUEST_PATH] per line or space separated
#   CUA_SB_WAIT           readiness targets (default desktop; "none" skips)
#   CUA_SB_READY_TIMEOUT  seconds (default 600)
#   CUA_SB_RUNTIME        local containers: runc or gvisor (installs runsc)
#   CUA_SB_CREATE_ARGS    extra `cua sb create` flags (word split)
#   CUA_SB_CLAIM_TTL      cloud: claim lifetime without keep-alives (60m)
#   CUA_SB_GUEST_SETUP    shell script run as root in the sandbox before the doctor
#   CUA_SB_DOCTOR         expect (default: build identities and expectations),
#                         full, strict, or false
#   CUA_SB_DOCTOR_SKIP    guest check ids or groups to skip (comma separated)
#   CUA_SB_EXPECT         extra COMPONENT=WANT expectations
#   CUA_SB_RUN            bash script run on the host (CUA_SANDBOX=<ref>)
#   CUA_SB_GUEST_RUN      shell script run inside the sandbox
#   CUA_SB_KEEP           true: do not delete the sandbox
#   CUA_BIN               the cua CLI to use (else CUA_SB_CUA_VERSION)
#   CUA_SB_CUA_VERSION    source (default: build from this checkout) or a
#                         pinned cua release to install
#   CUA_SB_STATE          state and artifact directory
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$here/../../.." && pwd)"
phase="${1:-all}"

log() { printf '[cua-sandbox] %s\n' "$*" >&2; }
die() { printf '::error::%s\n' "$*" >&2; exit 1; }

output() {
    # A step output (and, locally, just a log line).
    if [ -n "${GITHUB_OUTPUT:-}" ]; then
        if [[ "$2" == *$'\n'* ]]; then
            local delim="EOF_$RANDOM$RANDOM"
            printf '%s<<%s\n%s\n%s\n' "$1" "$delim" "$2" "$delim" >>"$GITHUB_OUTPUT"
        else
            printf '%s=%s\n' "$1" "$2" >>"$GITHUB_OUTPUT"
        fi
    fi
    log "output $1=${2:0:200}"
}

# The sandbox name: stable across the steps of one action invocation, so
# cleanup finds it even when creation was cancelled half way.
default_name() {
    local job="${GITHUB_JOB:-local}" run="${GITHUB_RUN_ID:-$$}" attempt="${GITHUB_RUN_ATTEMPT:-1}"
    local raw="cua-ci-$run-$attempt-$job"
    raw="$(printf '%s' "$raw" | tr '[:upper:]' '[:lower:]' | tr -c 'a-z0-9-' '-' | sed 's/--*/-/g; s/-$//')"
    printf '%s' "${raw:0:56}"
}

name="${CUA_SB_NAME:-$(default_name)}"
state="${CUA_SB_STATE:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}/cua-sandbox-$name}"
mkdir -p "$state/artifacts"
on="${CUA_SB_ON:-local}"
case "$on" in
    local | docker | qemu | lume) ref="local:$name" ;;
    cloud | fleet) ref="cloud:$name"; on=cloud ;;
    # Contrib providers (a cua built with --features contrib and the
    # provider's key in the step env).
    e2b | daytona | modal) ref="$on:$name" ;;
    *) die "on: $on is not local, docker, qemu, lume, cloud, e2b, daytona or modal" ;;
esac

# Split a multi-line / space separated list into array $items.
split_list() {
    items=()
    local word
    for word in $1; do items+=("$word"); done
}

cua_path() { cat "$state/cua-path" 2>/dev/null || true; }

# ---------------------------------------------------------------- setup
install_cua() {
    local version="$1" os arch platform url dir
    os="$(uname -s)"; arch="$(uname -m)"
    case "$os/$arch" in
        Linux/x86_64) platform=linux-x64-gnu ;;
        Linux/aarch64 | Linux/arm64) platform=linux-arm64-gnu ;;
        Darwin/arm64) platform=darwin-arm64 ;;
        Darwin/x86_64) platform=darwin-x64 ;;
        *) die "no cua CLI release for $os/$arch; build it (cargo build -p cua-cli) and pass cua-bin" ;;
    esac
    dir="$state/cua-$version"
    mkdir -p "$dir"
    url="https://github.com/trycua/cua/releases/download/cua-sdk-v$version"
    log "installing cua $version ($platform)"
    curl -fsSL --retry 3 -o "$dir/cua.tar.gz" "$url/cua-cli-$version-$platform.tar.gz" \
        || die "cua $version is not released for $platform ($url); pin another cua-version or pass cua-bin"
    curl -fsSL --retry 3 -o "$dir/checksums.txt" "$url/checksums.txt" || die "no checksums.txt in $url"
    local want got
    want="$(awk -v f="cua-cli-$version-$platform.tar.gz" '$2 == f || $2 == "*"f {print $1}' "$dir/checksums.txt")"
    if command -v sha256sum >/dev/null 2>&1; then
        got="$(sha256sum "$dir/cua.tar.gz" | cut -d' ' -f1)"
    else
        got="$(shasum -a 256 "$dir/cua.tar.gz" | cut -d' ' -f1)"
    fi
    if [ -z "$want" ] || [ "$want" != "$got" ]; then
        die "cua $version archive checksum mismatch"
    fi
    tar -xzf "$dir/cua.tar.gz" -C "$dir" cua
    printf '%s' "$dir/cua"
}

# The cua CLI from the action's own checkout (the ref in `uses: ...@ref`).
build_cua() {
    command -v cargo >/dev/null || die "cua-version: source needs a Rust toolchain (dtolnay/rust-toolchain) or pass cua-bin"
    if ! command -v protoc >/dev/null && command -v apt-get >/dev/null; then
        sudo -n apt-get install -y -qq --no-install-recommends protobuf-compiler >/dev/null
    fi
    log "building cua from $(git -C "$repo_root" rev-parse --short HEAD 2>/dev/null || echo "$repo_root")"
    (cd "$repo_root/libs/cua" && CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" cargo build --locked -p cua-cli >&2)
    printf '%s' "${CARGO_TARGET_DIR:-$repo_root/libs/cua/target}/debug/cua"
}

setup() {
    local cua
    if [ -n "${CUA_BIN:-}" ]; then
        cua="$(cd "$(dirname "$CUA_BIN")" && pwd)/$(basename "$CUA_BIN")"
        [ -x "$cua" ] || die "cua-bin $CUA_BIN is not an executable"
    elif [ "${CUA_SB_CUA_VERSION:-source}" = source ]; then
        cua="$(build_cua)"
    elif [ -n "${CUA_SB_CUA_VERSION:-}" ]; then
        cua="$(install_cua "$CUA_SB_CUA_VERSION")"
    else
        die "set cua-bin, or cua-version to source or a release"
    fi
    printf '%s' "$cua" >"$state/cua-path"
    log "cua: $cua ($("$cua" --version 2>/dev/null || echo unknown version))"

    # PATH isolation: our bin dir comes first and shadows cua, cua-driver and
    # cua-spacesd, so a copy installed on the runner can never be picked up.
    # cua-driver resolves to the overlaid build when one is injected, else to
    # a stub that fails loudly.
    local bin="$state/bin" overlay_driver=""
    mkdir -p "$bin"
    ln -sf "$cua" "$bin/cua"
    split_list "${CUA_SB_OVERLAY:-}"
    local spec
    for spec in ${items[@]+"${items[@]}"}; do
        case "$spec" in cua-driver=*)
            overlay_driver="${spec#cua-driver=}"; overlay_driver="${overlay_driver%%:/*}" ;;
        esac
    done
    local tool
    for tool in cua-driver cua-spacesd; do
        rm -f "$bin/$tool"
        if [ "$tool" = cua-driver ] && [ -n "$overlay_driver" ] && [ -f "$overlay_driver" ]; then
            ln -sf "$(cd "$(dirname "$overlay_driver")" && pwd)/$(basename "$overlay_driver")" "$bin/$tool"
        else
            # shellcheck disable=SC2016 # $CUA_SANDBOX expands when the stub runs
            printf '#!/bin/sh\necho "%s is isolated in this job: the build under test runs in the sandbox (cua sb exec \\"$CUA_SANDBOX\\" %s ...)" >&2\nexit 127\n' "$tool" "$tool" >"$bin/$tool"
            chmod +x "$bin/$tool"
        fi
    done
    local runner_copy
    for tool in cua-driver cua-spacesd; do
        runner_copy="$(PATH="${PATH}" command -v "$tool" 2>/dev/null || true)"
        [ -n "$runner_copy" ] && log "shadowing $tool on the runner PATH ($runner_copy)"
    done
    printf '%s' "$bin:$PATH" >"$state/path"

    # Local runtimes: KVM for QEMU, runsc for gVisor.
    if [ "$on" != cloud ] && [ -e /dev/kvm ] && [ ! -w /dev/kvm ] && command -v sudo >/dev/null; then
        if sudo -n chmod 666 /dev/kvm 2>/dev/null; then log "KVM enabled"; fi
    fi
    if [ "${CUA_SB_RUNTIME:-}" = gvisor ] && ! command -v runsc >/dev/null 2>&1; then
        "$repo_root/scripts/images/install-gvisor.sh"
    fi

    output name "$name"
    output ref "$ref"
    output state "$state"
    output artifacts-dir "$state/artifacts"
    output path "$bin:$PATH"
}

# ---------------------------------------------------------------- create
create() {
    local cua; cua="$(cua_path)"; [ -n "$cua" ] || die "run the setup phase first"
    local args=(sb create "${CUA_SB_IMAGE:?image is required}" --name "$name" --json)
    case "$on" in
        cloud) args+=(--on cloud --claim-ttl "${CUA_SB_CLAIM_TTL:-60m}") ;;
        # The platform's lifetime backstop, like a cloud claim TTL.
        e2b | daytona | modal) args+=(--on "$on" --claim-ttl "${CUA_SB_CLAIM_TTL:-60m}") ;;
        *) args+=(--on "$on") ;;
    esac
    [ -n "${CUA_SB_RUNTIME:-}" ] && args+=(--runtime "$CUA_SB_RUNTIME")
    local wait="${CUA_SB_WAIT-desktop}"
    if [ "$wait" != none ]; then
        split_list "$wait"
        local w; for w in ${items[@]+"${items[@]}"}; do args+=(--wait "$w"); done
    fi
    args+=(--ready-timeout "${CUA_SB_READY_TIMEOUT:-600}")
    split_list "${CUA_SB_OVERLAY:-}"
    local o; for o in ${items[@]+"${items[@]}"}; do args+=(--overlay "$o"); done
    if [ -n "${CUA_SB_CREATE_ARGS:-}" ]; then
        split_list "$CUA_SB_CREATE_ARGS"
        args+=(${items[@]+"${items[@]}"})
    fi
    log "cua ${args[*]}"
    : >"$state/created"
    "$cua" "${args[@]}" >"$state/create.json" 2> >(tee "$state/artifacts/create.log" >&2)
    local created
    created="$(cat "$state/create.json")"
    printf '%s\n' "$created" >"$state/artifacts/sandbox.json"
    # Overlay expectations: exactly the builds that were injected.
    printf '%s' "$created" | python3 -c '
import json, sys
v = json.load(sys.stdin)
print("\n".join(o["expect"] for o in v.get("overlays", [])))' >"$state/expect-overlays"
    output overlays "$(printf '%s' "$created" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin).get("overlays", [])))')"
    output urls "$(printf '%s' "$created" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin).get("endpoints", {})))')"
    output env-url "$(printf '%s' "$created" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("endpoints", {}).get("env", ""))')"
    # The browser desktop viewer: the `viewer` service (cua-spacesd-html5,
    # `cua sb view`) when the image serves it, else the image's existing
    # web display service. Never the raw RFB port.
    output viewer-url "$(printf '%s' "$created" | python3 -c '
import json, sys
e = json.load(sys.stdin).get("endpoints", {})
print(next((e[k] for k in ("viewer", "novnc", "display", "web") if k in e), ""))')"
}

# ---------------------------------------------------------------- doctor
doctor() {
    local mode="${CUA_SB_DOCTOR:-expect}" cua; cua="$(cua_path)"
    if [ "$mode" = false ]; then output doctor-status skipped; return 0; fi
    local args=(doctor "$ref" --no-host --out "$state/artifacts/doctor.json" --junit "$state/artifacts/doctor.xml")
    case "$mode" in
        expect) args+=(--only build) ;;
        full) ;;
        strict) args+=(--strict) ;;
        *) die "doctor: $mode is not expect, full, strict or false" ;;
    esac
    [ -n "${CUA_SB_DOCTOR_SKIP:-}" ] && args+=(--skip "$CUA_SB_DOCTOR_SKIP")
    local e
    split_list "$(cat "$state/expect-overlays" 2>/dev/null || true) ${CUA_SB_EXPECT:-}"
    for e in ${items[@]+"${items[@]}"}; do args+=(--expect "$e"); done
    log "cua ${args[*]}"
    if "$cua" "${args[@]}" | tee "$state/artifacts/doctor.txt"; then
        output doctor-status pass
    else
        output doctor-status fail
        die "cua doctor failed for $ref (see doctor.txt; stale builds show as expect.* failures)"
    fi
}

# ---------------------------------------------------------------- run
run_host() {
    [ -n "${CUA_SB_RUN:-}" ] || return 0
    local cua; cua="$(cua_path)"
    local driver_env=()
    if [ -L "$state/bin/cua-driver" ]; then
        driver_env=(CUA_DRIVER_BINARY="$state/bin/cua-driver" CUA_TEST_DRIVER_BIN="$state/bin/cua-driver")
    fi
    log "run (host): ${CUA_SB_RUN:0:200}"
    env PATH="$(cat "$state/path")" CUA_BIN="$cua" CUA_SANDBOX="$ref" CUA_SANDBOX_NAME="$name" \
        CUA_SANDBOX_ARTIFACTS="$state/artifacts" ${driver_env[@]+"${driver_env[@]}"} \
        bash -euo pipefail -c "$CUA_SB_RUN"
}

guest_setup() {
    [ -n "${CUA_SB_GUEST_SETUP:-}" ] || return 0
    local cua container=""; cua="$(cua_path)"
    log "guest setup (root): ${CUA_SB_GUEST_SETUP:0:200}"
    # Root: local containers through the engine (sudo cannot work under
    # gVisor), everything else through sudo in the guest.
    container="$("$cua" sb info "$ref" --json 2>/dev/null | python3 -c '
import json, sys
print(json.load(sys.stdin).get("provider_details", {}).get("container_id", ""))' 2>/dev/null || true)"
    if [ -n "$container" ] && command -v docker >/dev/null 2>&1; then
        docker exec -u 0 "$container" /bin/sh -c "$CUA_SB_GUEST_SETUP" 2>&1 | tee "$state/artifacts/guest-setup.log"
    else
        "$cua" sb shell "$ref" "sudo -n /bin/sh -c $(printf '%q' "$CUA_SB_GUEST_SETUP")" 2>&1 | tee "$state/artifacts/guest-setup.log"
    fi
    return "${PIPESTATUS[0]}"
}

run_guest() {
    [ -n "${CUA_SB_GUEST_RUN:-}" ] || return 0
    local cua; cua="$(cua_path)"
    log "run (guest): ${CUA_SB_GUEST_RUN:0:200}"
    "$cua" sb shell "$ref" "$CUA_SB_GUEST_RUN"
}

# ---------------------------------------------------------------- collect
collect() {
    local cua; cua="$(cua_path)"
    if [ -z "$cua" ] || [ ! -e "$state/created" ]; then log "nothing to collect"; return 0; fi
    local a="$state/artifacts"
    "$cua" sb info "$ref" --json >"$a/info.json" 2>>"$a/collect.log" || true
    "$cua" sb logs "$ref" -n 2000 >"$a/console.log" 2>>"$a/collect.log" || true
    "$cua" sb logs "$ref" -n 2000 --source guest >"$a/guest.log" 2>>"$a/collect.log" || true
    # shellcheck disable=SC2016 # expands in the guest
    "$cua" sb exec "$ref" 'for f in /var/log/supervisor/*.log; do echo "==> $f"; sudo -n tail -n 400 "$f" 2>/dev/null || tail -n 400 "$f"; done; ls /var/lib/cua/overlays 2>/dev/null && cat /var/lib/cua/overlays/*.json 2>/dev/null' \
        >"$a/services.log" 2>>"$a/collect.log" || true
    "$cua" sb screenshot "$ref" -o "$a/screenshot.png" >>"$a/collect.log" 2>&1 || true
    log "artifacts in $a: $(find "$a" -maxdepth 1 -type f -exec basename {} \; | tr '\n' ' ')"
}

# ---------------------------------------------------------------- cleanup
cleanup() {
    local cua; cua="$(cua_path)"
    if [ "${CUA_SB_KEEP:-false}" = true ]; then
        log "keep: true, leaving $ref running"
        return 0
    fi
    if [ -z "$cua" ] || [ ! -e "$state/created" ]; then log "no sandbox to delete"; return 0; fi
    local i
    for i in 1 2 3; do
        if "$cua" sb rm "$ref" --force >/dev/null 2>>"$state/artifacts/collect.log"; then
            log "deleted $ref"
            return 0
        fi
        # Already gone counts as cleaned up.
        if ! "$cua" sb info "$ref" --json >/dev/null 2>&1; then
            log "$ref is gone"
            return 0
        fi
        sleep "$((i * 5))"
    done
    die "could not delete $ref"
}

case "$phase" in
    setup) setup ;;
    create) create ;;
    guest-setup) guest_setup ;;
    doctor) doctor ;;
    run) run_host ;;
    guest-run) run_guest ;;
    collect) collect ;;
    cleanup) cleanup ;;
    all)
        status=0
        trap 'collect || true; cleanup || true' EXIT
        trap 'exit 130' INT TERM
        setup
        create
        guest_setup
        doctor || status=$?
        [ "$status" = 0 ] && { run_host || status=$?; }
        [ "$status" = 0 ] && { run_guest || status=$?; }
        exit "$status"
        ;;
    *) die "unknown phase $phase" ;;
esac
