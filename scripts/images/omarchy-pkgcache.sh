#!/usr/bin/env bash
# Moves the Omarchy image's pacman package cache between a plain directory
# (what GitHub Actions caches) and the BuildKit cache mount the Dockerfiles use
# (id omarchy-pacman-pkg on /var/cache/pacman/pkg).
#
#   omarchy-pkgcache.sh seed DIR   copy DIR/*.pkg.tar.* into the mount (before the build)
#   omarchy-pkgcache.sh save DIR   copy the mount into DIR, dropping files older than 21 days
#
# Why: archive.archlinux.org is the only host with old package versions and it
# can stall; with this cache each package version is downloaded once. A cache
# miss or an empty cache only means pacman downloads from the archive as before.
# Cached files are never trusted blindly: pacman matches them against the dated
# snapshot database and checks signatures.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DOCKERFILE="$HERE/../../libs/images/omarchy/Dockerfile"
mode="${1:?seed|save}"; dir="${2:?directory}"
mkdir -p "$dir"
base="$(sed -n 's/^ARG ARCH_BASE=//p' "$DOCKERFILE")"
[ -n "$base" ] || { echo "no ARG ARCH_BASE in $DOCKERFILE" >&2; exit 1; }
mount="--mount=type=cache,id=omarchy-pacman-pkg,target=/var/cache/pacman/pkg,sharing=locked"
case "$mode" in
seed)
    docker buildx build --build-arg "BASE=$base" --build-arg "NOW=$(date +%s)" --progress=plain -f - "$dir" <<DF
# syntax=docker/dockerfile:1.7
ARG BASE
FROM \${BASE}
ARG NOW
RUN $mount --mount=type=bind,target=/seed \\
    sh -c 'for f in /seed/*.pkg.tar.* ; do [ -e "\$f" ] && cp -pn "\$f" /var/cache/pacman/pkg/ ; done; ls /var/cache/pacman/pkg | wc -l'
DF
    ;;
save)
    ctx="$(mktemp -d)"; trap 'rmdir "$ctx"' EXIT
    docker buildx build --build-arg "BASE=$base" --build-arg "NOW=$(date +%s)" --progress=plain -f - --output "type=local,dest=$dir" "$ctx" <<DF
# syntax=docker/dockerfile:1.7
ARG BASE
FROM \${BASE} AS read
ARG NOW
RUN $mount sh -c 'mkdir /out && find /var/cache/pacman/pkg -maxdepth 1 -name "*.pkg.tar.*" -mtime -21 -exec cp -p {} /out/ \;'
FROM scratch
COPY --from=read /out/ /
DF
    # The export merges into DIR; age out versions nothing has needed for 3 weeks.
    find "$dir" -maxdepth 1 -name '*.pkg.tar.*' -mtime +21 -delete
    ;;
*) echo "usage: $0 seed|save DIR" >&2; exit 2 ;;
esac
echo "omarchy-pkgcache $mode: $(find "$dir" -maxdepth 1 -name '*.pkg.tar.*' | wc -l) packages in $dir"
