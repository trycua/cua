#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Runs cua-volume's S3 tests (and, with --bench, the drive benchmark) against
# MinIO in Docker. The container gets 1 GiB of memory, listens on loopback
# only, and is always removed. MinIO no longer publishes images on Docker
# Hub; the default is Chainguard's build of upstream MinIO (override with
# MINIO_IMAGE).
#
#   tests/run-minio.sh [--bench] [--keep]
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE="$(cd "$HERE/../../.." && pwd)" # libs/cua
MINIO_IMAGE="${MINIO_IMAGE:-cgr.dev/chainguard/minio:latest}"
BENCH=0
KEEP=0
for a in "$@"; do
    case "$a" in
        --bench) BENCH=1 ;;
        --keep) KEEP=1 ;;
        *) echo "unknown option $a" >&2; exit 2 ;;
    esac
done
NAME="cua-volume-minio-$$"
KEY="cua$(head -c 6 /dev/urandom | od -An -tx1 | tr -d ' \n')"
SECRET="$(head -c 18 /dev/urandom | od -An -tx1 | tr -d ' \n')"
cleanup() { [ "$KEEP" = 1 ] || docker rm -f "$NAME" >/dev/null 2>&1 || true; }
trap cleanup EXIT
docker run -d --name "$NAME" --memory=1g --memory-swap=1g -p 127.0.0.1::9000 \
    -e MINIO_ROOT_USER="$KEY" -e MINIO_ROOT_PASSWORD="$SECRET" \
    --tmpfs /data:rw,size=512m,uid=65532 "$MINIO_IMAGE" server /data >/dev/null
PORT="$(docker port "$NAME" 9000/tcp | head -1 | sed 's/.*://')"
for _ in $(seq 1 60); do
    curl -fs "http://127.0.0.1:$PORT/minio/health/ready" >/dev/null 2>&1 && break
    sleep 1
done
echo "==> MinIO on 127.0.0.1:$PORT"
cd "$WORKSPACE"
export CUA_DRIVE_S3_TEST_ENDPOINT="http://127.0.0.1:$PORT"
export CUA_DRIVE_S3_TEST_ACCESS_KEY="$KEY" CUA_DRIVE_S3_TEST_SECRET_KEY="$SECRET"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
timeout 900 cargo test -p cua-volume --features s3 -- --test-threads=2
# The daemon's config path to the same bucket, keys from the environment,
# in a throwaway cua home with the file credential store.
CUA_TMP_HOME="$(mktemp -d)"
CUA_HOME="$CUA_TMP_HOME" CUA_CREDENTIAL_STORE=file CUA_ENV_TEST_SANDBOX=1 \
    CUA_DRIVE_S3_ACCESS_KEY_ID="$KEY" CUA_DRIVE_S3_SECRET_ACCESS_KEY="$SECRET" \
    timeout 900 cargo test -p cua-daemon --features drive-s3 --lib drive -- --test-threads=2
rm -rf "$CUA_TMP_HOME"
if [ "$BENCH" = 1 ]; then
    timeout 1800 cargo run --release -p cua-volume --features s3 --example bench
fi
