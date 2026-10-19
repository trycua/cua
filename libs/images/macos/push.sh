#!/usr/bin/env bash
# Push a built (stopped, sanitized) macOS tier VM as a new immutable pin of
# ghcr.io/trycua/macos, and print its digest. build.sh --push and the CD
# workflow use it; floating tags move separately (check-tag-safety.sh
# --moving, then crane tag).
#
#   libs/images/macos/push.sh VM TAG
#
# TAG is a pin: ^26(-slim|-xcode(-[0-9.]+)?)?-<yyyymmdd>-<sha7>$, or that pin
# plus -raw: the un-annotated push that annotate.sh re-publishes under the pin
# (what cd-image-macos.yml does).
# Credentials: GITHUB_USERNAME / GITHUB_TOKEN (write:packages).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPOSITORY=ghcr.io/trycua/macos
VM="${1:?vm}" TAG="${2:?tag}"
[[ "$TAG" =~ ^26(-slim|-xcode(-[0-9.]+)?)?-[0-9]{8}-[0-9a-f]{7}(-raw)?$ ]] ||
    { echo "$TAG is not a macOS pin (26[-slim|-xcode[-X.Y]]-<yyyymmdd>-<sha7>[-raw])" >&2; exit 2; }
: "${GITHUB_USERNAME:?lume push needs GITHUB_USERNAME}" "${GITHUB_TOKEN:?lume push needs GITHUB_TOKEN}"
"$HERE/../../../scripts/images/check-tag-safety.sh" "$REPOSITORY:$TAG" >&2
# Retried: a dropped connection fails the whole push, and ghcr keeps the
# blobs already uploaded, so a retry resumes cheaply.
for attempt in 1 2 3; do
    lume push "$VM" "macos:$TAG" --registry ghcr.io --organization trycua --chunk-size-mb 512 >&2 && break
    [ "$attempt" = 3 ] && { echo "lume push failed 3 times" >&2; exit 1; }
    echo "push attempt $attempt failed; retrying in 30 s" >&2; sleep 30
done
# A push that lost disk chunks still writes a manifest: refuse to hand on
# (tag, promote) an image whose chunks leave part of the disk out.
crane manifest "$REPOSITORY:$TAG" | python3 "$HERE/tools/disk-coverage.py" - >&2 ||
    { echo "$REPOSITORY:$TAG is incomplete; push it again (lume push resumes)" >&2; exit 1; }
crane digest "$REPOSITORY:$TAG"
