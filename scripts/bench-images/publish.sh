#!/usr/bin/env bash
# Publish a benchmark image: two cross-linked indexes, one per variant.
#
#   publish.sh ID [--stamp YYYYMMDD-sha7] [--dry-run] [--work DIR]
#              [--arch A | --indexes-only] [--resume]      stage: immutable pins
#   publish.sh ID --stamp S --promote                      move <ver>, <ver>-disk
#
# Staging pushes only immutable pins and writes lib/images/bench/ID/lock.json.
# --promote moves the floating tags to a staged stamp, and only when doctor
# evidence (scripts/bench-images/smoke.py, `cua-spacesd doctor --strict`) has
# passed for that stamp's exact index digests: every arch, both variants
# (CUA_BENCH_EVIDENCE, default libs/images/bench/ID/evidence).
#
# --resume finishes an interrupted run with the same --stamp: pins that
# already exist are accepted only when they hold exactly the content this
# run pushes (check-tag-safety.sh --digest), never re-pointed.
# --arch A pushes only that arch's two children (a CI job per runner arch);
# --indexes-only reads every arch's children back from their immutable tags
# and pushes the indexes and floating tags (the final CI job). Without either,
# one machine does it all.
#
# Inputs (from build-qcow2.sh / build-docker.sh), per arch in bench.json:
#   rootfs  docker image cua-e2e-local/bench-ID:docker-local-<arch>, or an OCI
#           layout at WORK/ID/out/<arch>/rootfs-layout (qcow2 sources)
#   disk    WORK/ID/out/<arch>/disk.img
# Pushes, in this order (every push is checked by
# scripts/images/check-tag-safety.sh first; only ghcr.io/trycua/bench-* repos
# are accepted):
#   <repo>:<ver>-<stamp>-<arch>        rootfs child           (immutable)
#   <repo>:<ver>-disk-<stamp>-<arch>   containerDisk child    (immutable)
#   <repo>:<ver>-disk-<stamp>          containerDisk index    (immutable; KubeVirt/QEMU pull this),
#                                      ai.cua.image.variants {"rootfs": "<repo>:<ver>-<stamp>"}
#   <repo>:<ver>-<stamp>               rootfs index           (immutable; docker/gVisor),
#                                      ai.cua.image.variants {"containerdisk": "<repo>@<disk index>"}
#   <repo>:<ver>-disk, <repo>:<ver>    floating version tags (bench repos only; --promote)
# One index per variant: docker and containerd take the first child matching
# the platform and ignore annotations, and ctr pulls every matching child, so
# a single multi-variant index over-pulls.
# and writes libs/images/bench/ID/lock.json with every digest.
#
# A new ghcr.io/trycua package is private. "public" benchmarks must be made
# public once by an org admin in the package settings (GitHub has no API for
# package visibility); "private" ones stay private, and are never linked to
# the public repository.
# Credentials: CUA_BENCH_REGISTRY_CONFIG, a docker config directory used by
# crane, oras and the tag-safety check only (so the docker CLI keeps its own
# config and context), else DOCKER_CONFIG / ~/.docker. Never printed.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$HERE/../.." && pwd)"
CHECK="$REPO_ROOT/scripts/images/check-tag-safety.sh"
ID="${1:?benchmark id}"; shift
STAMP="$(date -u +%Y%m%d)-$(git -C "$HERE" rev-parse --short=7 HEAD)"
DRY=0 PROMOTE=0 ONLY_ARCH="" INDEXES_ONLY=0 RESUME=0
WORK_ROOT="${CUA_BENCH_WORK:-$HOME/.cache/cua-bench-images}"
while [ $# -gt 0 ]; do
    case "$1" in
        --stamp) STAMP="$2"; shift 2 ;;
        --dry-run) DRY=1; shift ;;
        --promote) PROMOTE=1; shift ;;
        --work) WORK_ROOT="$2"; shift 2 ;;
        --arch) ONLY_ARCH="$2"; shift 2 ;;
        --indexes-only) INDEXES_ONLY=1; shift ;;
        --resume) RESUME=1; shift ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
[[ "$STAMP" =~ ^[0-9]{8}-[0-9a-f]{7}$ ]] || { echo "bad stamp $STAMP" >&2; exit 2; }
for t in crane jq docker; do command -v "$t" >/dev/null || { echo "missing $t" >&2; exit 2; }; done
bench() { python3 "$HERE/bench.py" "$@"; }
log() { echo "[publish $ID $(date +%T)] $*" >&2; }
REG_CFG="${CUA_BENCH_REGISTRY_CONFIG:-${DOCKER_CONFIG:-}}"
ORAS_CFG=()
[ -n "$REG_CFG" ] && ORAS_CFG=(--registry-config "$REG_CFG/config.json")
if [ -n "${CUA_BENCH_REGISTRY_CONFIG:-}" ]; then
    crane() { DOCKER_CONFIG="$CUA_BENCH_REGISTRY_CONFIG" command crane "$@"; }
    check_tags() { DOCKER_CONFIG="$CUA_BENCH_REGISTRY_CONFIG" "$CHECK" "$@"; }
else
    check_tags() { "$CHECK" "$@"; }
fi

REPO="$(bench get "$ID" repository)"
VER="$(bench get "$ID" version)"
RED="$(bench get "$ID" redistribution)"
case "$REPO" in ghcr.io/trycua/bench-*) ;; *) echo "refusing $REPO: bench images go to ghcr.io/trycua/bench-* only" >&2; exit 2 ;; esac
case "$RED" in public|private-only) ;; *) echo "refusing: redistribution=$RED is never pushed (build on the user's side)" >&2; exit 2 ;; esac
ALL_ARCHES="$(bench get "$ID" arch | jq -r '.[]')"
VIS="$(bench get "$ID" visibility)"
ARCHES="$ALL_ARCHES"
if [ -n "$ONLY_ARCH" ]; then
    grep -qx "$ONLY_ARCH" <<<"$ALL_ARCHES" || { echo "$ID has no arch $ONLY_ARCH" >&2; exit 2; }
    ARCHES="$ONLY_ARCH"
fi
[ "$INDEXES_ONLY" = 1 ] && ARCHES=""
WORK="$WORK_ROOT/$ID/publish-$STAMP"
mkdir -p "$WORK"

if [ "$PROMOTE" = 1 ]; then
    # Floating tags follow doctor evidence for the exact staged digests.
    LOCK="$REPO_ROOT/libs/images/bench/$ID/lock.json"
    [ "$(jq -r .stamp "$LOCK" 2>/dev/null)" = "$STAMP" ] || { echo "lock.json is not stamp $STAMP (stage it first)" >&2; exit 2; }
    index="$(jq -r .index.digest "$LOCK")" disk_index="$(jq -r .disk_index.digest "$LOCK")"
    EVID="${CUA_BENCH_EVIDENCE:-$REPO_ROOT/libs/images/bench/$ID/evidence}"
    missing=()
    for a in $ALL_ARCHES; do
        for pair in "rootfs=$REPO@$index" "containerdisk=$REPO@$disk_index"; do
            v="${pair%%=*}" want="${pair#*=}"
            ok="$(jq -s --arg v "$v" --arg a "$a" --arg want "$want" \
                '[.[] | select(.variant == $v and .arch == $a and .pinned_ref == $want and .ok == true and .doctor == "pass")] | length' \
                "$EVID"/*.json 2>/dev/null || echo 0)"
            [ "${ok:-0}" -gt 0 ] || missing+=("$v/$a ($want)")
        done
    done
    if [ ${#missing[@]} -gt 0 ]; then
        echo "REFUSED: no passing doctor evidence in $EVID for:" >&2; printf '  %s\n' "${missing[@]}" >&2; exit 1
    fi
    [ "$DRY" = 1 ] && { log "evidence complete; dry run, not promoting"; exit 0; }
    check_tags --moving "$REPO:$VER-disk" "$REPO:$VER"
    crane tag "$REPO@$disk_index" "$VER-disk"
    crane tag "$REPO@$index" "$VER"
    [ "$(crane digest "$REPO:$VER")" = "$index" ] && [ "$(crane digest "$REPO:$VER-disk")" = "$disk_index" ] \
        || { echo "floating tags do not point at the staged indexes" >&2; exit 1; }
    jq '.promoted = true' "$LOCK" >"$LOCK.tmp" && mv "$LOCK.tmp" "$LOCK"
    log "promoted $REPO:$VER -> $index, $REPO:$VER-disk -> $disk_index"
    exit 0
fi

# Every ref this run creates must be new (or already hold the same content).
pins=()
[ -z "$ONLY_ARCH" ] && pins+=("$REPO:$VER-$STAMP" "$REPO:$VER-disk-$STAMP")
for a in $ARCHES; do pins+=("$REPO:$VER-$STAMP-$a" "$REPO:$VER-disk-$STAMP-$a"); done
[ ${#pins[@]} -eq 0 ] || [ "$RESUME" = 1 ] || check_tags "${pins[@]}"

# ghcr has no API to set a package's visibility. Before a private
# benchmark's first large push,
# a tiny probe image (pushed by digest, untagged) creates the package and its
# visibility is read back; anything but "private" aborts before real content
# is pushed.
package_visibility() {
    # gh prints the error body to stdout on a 404, so only trust a success.
    local out
    if out="$(gh api "/orgs/trycua/packages/container/${REPO#ghcr.io/trycua/}" --jq .visibility 2>/dev/null)"; then
        echo "$out"
    else
        echo unknown
    fi
}
if [ "$VIS" = private ] && [ "$DRY" = 0 ]; then
    command -v gh >/dev/null || { echo "gh is needed to verify the private package's visibility" >&2; exit 2; }
    cur="$(package_visibility)"
    if [ "$cur" = unknown ]; then
        log "creating the private package with a probe image"
        printf '{"config":{"Labels":{"ai.cua.image.probe":"visibility"}}}' >"$WORK/probe.json"
        bench layout "$WORK/probe" --platform linux/amd64 --config "$WORK/probe.json" >/dev/null
        crane push "$WORK/probe" "$REPO@$(jq -r '.manifests[0].digest' "$WORK/probe/index.json")" >&2
        for _ in 1 2 3 4 5 6; do cur="$(package_visibility)"; [ "$cur" != unknown ] && break; sleep 5; done
    fi
    [ "$cur" = private ] || { echo "REFUSED: $REPO is '$cur', not private; fix it in the org's package settings" >&2; exit 1; }
    log "package visibility: private"
fi

# push_layout DIR REF -> digest (crane pushes the single manifest of an OCI layout)
push_layout() {
    set -e
    local dir="$1" ref="$2" want
    want="$(jq -r '.manifests[0].digest' "$dir/index.json")"
    if [ "$DRY" = 1 ]; then echo "$want"; return; fi
    check_tags --digest "$want" "$ref" >&2
    crane push "$dir" "$ref" >&2
    local got; got="$(crane digest "$ref")"
    [ "$got" = "$want" ] || { echo "pushed $ref but it is $got, not $want" >&2; exit 1; }
    echo "$want"
}
# push_index FILE REF -> digest
push_index() {
    set -e
    local file="$1" ref="$2" want
    want="sha256:$(shasum -a 256 "$file" | cut -d' ' -f1)"
    if [ "$DRY" = 1 ]; then echo "$want"; return; fi
    check_tags --digest "$want" "$ref" >&2
    crane manifest "$ref" >/dev/null 2>&1 && [ "$(crane digest "$ref")" = "$want" ] && { echo "$want"; return; }
    oras manifest push ${ORAS_CFG[@]+"${ORAS_CFG[@]}"} --media-type application/vnd.oci.image.index.v1+json "$ref" "$file" >&2
    local got; got="$(crane digest "$ref")"
    [ "$got" = "$want" ] || { echo "pushed $ref but it is $got, not $want" >&2; exit 1; }
    echo "$want"
}
desc() { # DIR PLATFORM -> descriptor json file
    jq --arg p "$2" '.manifests[0] + {platform: {architecture: ($p|split("/")[1]), os: "linux"}}' "$1/index.json" >"$1.desc.json"
    echo "$1.desc.json"
}

rootfs_descs=() disk_descs=() record_children="[]"
for a in $ARCHES; do
    src="cua-e2e-local/bench-$ID:docker-local-$a"
    layout="$WORK_ROOT/$ID/out/$a/rootfs-layout"
    disk="$WORK_ROOT/$ID/out/$a/disk.img"
    [ -f "$disk" ] || { echo "missing $disk (build it first)" >&2; exit 1; }
    rm -rf "$WORK/rootfs-$a" "$WORK/rootfs-$a.tar"
    if [ -f "$layout/index.json" ]; then
        log "rootfs $a: OCI layout $layout"
        cp -R "$layout" "$WORK/rootfs-$a"
    else
        docker image inspect "$src" >/dev/null || { echo "missing $src or $layout (build it first)" >&2; exit 1; }
        log "rootfs $a: docker save $src"
        docker save --platform "linux/$a" -o "$WORK/rootfs-$a.tar" "$src"
        label_args=(); while IFS= read -r l; do label_args+=(--label "$l"); done < <(bench labels "$ID" --variant rootfs --stamp "$STAMP" | jq -r 'to_entries[] | "\(.key)=\(.value)"')
        drop=(); [ "$VIS" = private ] && drop=(--drop-label org.opencontainers.image.source)
        bench docker-layout "$WORK/rootfs-$a.tar" "$WORK/rootfs-$a" --platform "linux/$a" "${label_args[@]}" ${drop[@]+"${drop[@]}"} >/dev/null
        rm -f "$WORK/rootfs-$a.tar"
    fi
    rd="$(push_layout "$WORK/rootfs-$a" "$REPO:$VER-$STAMP-$a")"
    log "  $REPO:$VER-$STAMP-$a -> $rd"
    rootfs_descs+=(--rootfs "$(desc "$WORK/rootfs-$a" "linux/$a")")

    log "containerdisk $a: $(du -h "$disk" | cut -f1) $disk"
    rm -rf "$WORK/disk-$a"; mkdir -p "$WORK/disk-$a.layer"
    bench disk-layer "$disk" "$WORK/disk-$a.layer/disk.tar" >/dev/null
    jq -n --argjson l "$(bench labels "$ID" --variant containerdisk --stamp "$STAMP")" '{config: {Labels: $l}}' >"$WORK/disk-$a.config.json"
    bench layout "$WORK/disk-$a" --platform "linux/$a" --config "$WORK/disk-$a.config.json" \
        --layer "$WORK/disk-$a.layer/disk.tar.json" >/dev/null
    dd="$(push_layout "$WORK/disk-$a" "$REPO:$VER-disk-$STAMP-$a")"
    log "  $REPO:$VER-disk-$STAMP-$a -> $dd"
    rm -rf "$WORK/disk-$a.layer" "$WORK/disk-$a/blobs"
    disk_descs+=(--disk "$(desc "$WORK/disk-$a" "linux/$a")")
    record_children="$(jq -c --arg a "$a" --arg r "$rd" --arg d "$dd" '. + [{arch: $a, rootfs: $r, containerdisk: $d}]' <<<"$record_children")"
done

if [ -n "$ONLY_ARCH" ]; then
    jq -n --arg stamp "$STAMP" --argjson children "$record_children" '{stamp: $stamp, children: $children}' >"$WORK/record-$ONLY_ARCH.json"
    cat "$WORK/record-$ONLY_ARCH.json"
    exit 0
fi
if [ "$INDEXES_ONLY" = 1 ]; then
    # Children pushed by earlier --arch runs, read back from their pins.
    for a in $ALL_ARCHES; do
        for kind in rootfs disk; do
            tag="$VER-$STAMP-$a"; [ "$kind" = disk ] && tag="$VER-disk-$STAMP-$a"
            m="$(crane manifest "$REPO:$tag")"
            jq -n --arg d "$(crane digest "$REPO:$tag")" --argjson size "$(crane manifest "$REPO:$tag" | wc -c | tr -d ' ')" \
                --arg mt "$(jq -r .mediaType <<<"$m")" --arg a "$a" \
                '{mediaType: $mt, digest: $d, size: $size, platform: {architecture: $a, os: "linux"}}' >"$WORK/$kind-$a.desc.json"
            if [ "$kind" = rootfs ]; then rootfs_descs+=(--rootfs "$WORK/$kind-$a.desc.json"); else disk_descs+=(--disk "$WORK/$kind-$a.desc.json"); fi
        done
        record_children="$(jq -c --arg a "$a" --arg r "$(jq -r .digest "$WORK/rootfs-$a.desc.json")" --arg d "$(jq -r .digest "$WORK/disk-$a.desc.json")" '. + [{arch: $a, rootfs: $r, containerdisk: $d}]' <<<"$record_children")"
    done
fi

log "indexes"
bench index "$ID" --stamp "$STAMP" "${rootfs_descs[@]}" "${disk_descs[@]}" "$WORK/idx" >/dev/null
disk_index="$(push_index "$WORK/idx/disk-index.json" "$REPO:$VER-disk-$STAMP")"
log "  $REPO:$VER-disk-$STAMP -> $disk_index"
bench index "$ID" --stamp "$STAMP" "${rootfs_descs[@]}" "${disk_descs[@]}" --disk-digest "$disk_index" "$WORK/idx" >/dev/null
index="$(push_index "$WORK/idx/index.json" "$REPO:$VER-$STAMP")"
log "  $REPO:$VER-$STAMP -> $index"

log "staged; promote with: $0 $ID --stamp $STAMP --promote (after the doctor smokes pass)"

jq -n --arg stamp "$STAMP" --arg index "$index" --arg disk "$disk_index" --argjson children "$record_children" \
    --arg repo "$REPO" --arg ver "$VER" --argjson dry "$DRY" '{
      stamp: $stamp, dry_run: ($dry == 1),
      promoted: false,
      index: {ref: "\($repo):\($ver)-\($stamp)", digest: $index, floating: "\($repo):\($ver)"},
      disk_index: {ref: "\($repo):\($ver)-disk-\($stamp)", digest: $disk, floating: "\($repo):\($ver)-disk"},
      children: $children,
      pinned: "\($repo)@\($index)"}' >"$WORK/record.json"
if [ "$DRY" = 0 ]; then
    bench lock "$ID" --from "$WORK/record.json"
    command -v gh >/dev/null && log "package visibility now: $(package_visibility) (bench.json: $VIS)"
fi
cat "$WORK/record.json"
