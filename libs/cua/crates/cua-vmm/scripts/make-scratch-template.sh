#!/usr/bin/env bash
# Regenerates src/microvm/scratch-1g.ext4.sparse: the empty ext4 a macOS
# microVM's scratch disk starts from (macOS has no mkfs.ext4). It is 1 GiB;
# the runtime makes the disk file as large as the Space asks (sparse) and the
# guest init grows the filesystem to fill it at boot (EXT4_IOC_RESIZE_FS).
#
# Reproducible: fixed UUID, hash seed and time, e2fsprogs from a pinned
# Alpine image. Needs Docker (any Linux arm64 or x86_64 engine).
#
# The file: "CUASPARSE1", the image size (u64 LE), then a zlib stream of
# (u64 LE offset, 4096 bytes) records, one per block that is not all zeros.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
out="$here/src/microvm/scratch-1g.ext4.sparse"
# Under the repository: Docker on a Mac shares the home directory, not $TMPDIR.
work="$(mktemp -d "$here/.scratch-template.XXXXXX")"
trap 'rm -rf "$work"' EXIT
docker run --rm -v "$work":/w alpine:3.20@sha256:d9e853e87e55526f6b2917df91a2115c36dd7c696a35be12163d44e6e2a4b6bc sh -euc '
  apk add -q e2fsprogs >/dev/null
  truncate -s 1G /w/scratch.ext4
  E2FSPROGS_FAKE_TIME=1700000000 mkfs.ext4 -q -F -b 4096 -T default -m 0 -J size=64 \
    -U 6c75612d-7363-7261-7463-682d65787434 -L cua-scratch \
    -E lazy_itable_init=1,lazy_journal_init=1,hash_seed=6c75612d-7363-7261-7463-682d68617368 \
    /w/scratch.ext4
'
python3 - "$work/scratch.ext4" "$out" <<'PY'
import struct, sys, zlib
src, dst = sys.argv[1], sys.argv[2]
data = open(src, "rb").read()
body = bytearray()
for off in range(0, len(data), 4096):
    block = data[off:off + 4096]
    if block.count(0) != len(block):
        body += struct.pack("<Q", off) + block.ljust(4096, b"\0")
with open(dst, "wb") as f:
    f.write(b"CUASPARSE1" + struct.pack("<Q", len(data)) + zlib.compress(bytes(body), 9))
print(dst)
PY
