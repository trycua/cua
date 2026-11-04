# shellcheck shell=bash
# Share the job's sccache with a cargo build that runs inside `docker run`.
#
#   source scripts/ci/lib/docker-sccache.sh
#   docker_sccache_args           # fills the DOCKER_SCCACHE array
#   docker run "${DOCKER_SCCACHE[@]}" ... bash -c '...; docker_sccache_stop'
#
# The rust-cache action (.github/actions/rust-cache) restores an sccache
# directory and exports SCCACHE_DIR, SCCACHE_CACHE_SIZE and RUSTC_WRAPPER for
# the host. A build inside a container saw none of it and compiled cold on
# every run (the e2e reference images and the spacesd Docker lanes: 7 to 15
# minutes each). This mounts the host's sccache binary (the static musl build
# the action installs) and the restored directory into the container, so the
# container's rustc goes through the same cache and the action saves what it
# wrote. Paths inside the container are the same on every run, so its entries
# hit on the next one.
#
# Off (empty array) when there is no restored directory or the binary is not
# static: the build then compiles exactly as before. Set DOCKER_SCCACHE_OFF=1 to
# force that.
# shellcheck disable=SC2034 # read by the scripts that source this file
DOCKER_SCCACHE=()
docker_sccache_args() {
  DOCKER_SCCACHE=()
  [ "${DOCKER_SCCACHE_OFF:-}" = 1 ] && return 0
  [ -n "${SCCACHE_DIR:-}" ] && [ -d "$SCCACHE_DIR" ] || return 0
  local bin
  bin="$(command -v sccache 2>/dev/null)" || return 0
  bin="$(readlink -f "$bin")"
  # A dynamically linked binary would not run in another distribution's image.
  if ! file -L "$bin" 2>/dev/null | grep -q 'statically linked\|static-pie linked'; then
    echo "docker-sccache: $bin is not static; the container compiles without sccache" >&2
    return 0
  fi
  # shellcheck disable=SC2034
  DOCKER_SCCACHE=(
    -v "$bin:/usr/local/bin/sccache:ro"
    -v "$SCCACHE_DIR:/sccache"
    -e SCCACHE_DIR=/sccache
    -e "SCCACHE_CACHE_SIZE=${SCCACHE_CACHE_SIZE:-4G}"
    -e SCCACHE_IDLE_TIMEOUT=0
    -e RUSTC_WRAPPER=/usr/local/bin/sccache
    -e CARGO_INCREMENTAL=0
  )
  echo "docker-sccache: the container compiles through $SCCACHE_DIR"
}

# Inside the container, at the end of the build: flush the server's pending
# writes before the container exits, and print its hit rate. A no-op without
# sccache. Pass the body of this function into `bash -c` via
# "$(declare -f docker_sccache_stop)".
docker_sccache_stop() {
  if [ -n "${RUSTC_WRAPPER:-}" ] && command -v sccache >/dev/null 2>&1; then
    sccache --show-stats 2>/dev/null | grep -E 'Compile requests|Cache hits|Cache misses|Cache size' || true
    sccache --stop-server >/dev/null 2>&1 || true
  fi
}
