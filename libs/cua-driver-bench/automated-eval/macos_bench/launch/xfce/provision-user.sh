#!/bin/bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
#
# Per-user setup for the Linux XFCE rows (run as the desktop user, after provision-root.sh): the two drivers under
# test and the Electron apps of the task pack.
#
# Usage: ./provision-user.sh [TASKPACK_DIR]   (default: <repo>/libs/cua-driver-bench/taskpack)
# Drivers land in $CDB_HOME (default ~/cdb): cua-driver 0.38.0 and agent-desktop 1.2.0.
set -euo pipefail
CDB_HOME=${CDB_HOME:-$HOME/cdb}
HERE=$(cd "$(dirname "$0")" && pwd)
PACK=${1:-$(cd "$HERE/../../../../taskpack" && pwd)}
mkdir -p "$CDB_HOME" && cd "$CDB_HOME"

if [ ! -x cua-driver-rs-0.38.0-linux-x86_64/cua-driver ]; then
  curl -fsSL -o cd.tgz https://github.com/trycua/cua/releases/download/cua-driver-rs-v0.38.0/cua-driver-rs-0.38.0-linux-x86_64.tar.gz
  tar -xzf cd.tgz && rm cd.tgz
fi
mkdir -p agent-desktop && (cd agent-desktop && [ -d node_modules/agent-desktop ] || npm install --no-audit --no-fund --silent agent-desktop@1.2.0)
sha256sum cua-driver-rs-0.38.0-linux-x86_64/cua-driver agent-desktop/node_modules/agent-desktop/bin/agent-desktop-linux-x64

# Electron apps of CDB-G02..G04 (`npx electron .` in each app directory).
for a in cdb-s02/apps/lighthouse-desk cdb-s03/apps/ledger-lantern-mail cdb-s04/apps/rillwatch-cases cdb-s04/apps/margin-lantern-manual; do
  (cd "$PACK/tasks/shared/$a" && npm ci --no-audit --no-fund --silent && npx electron --version >/dev/null)
  if [ "${BENCH_NO_SANDBOX_SHIMS:-1}" = 1 ]; then
    d=$PACK/tasks/shared/$a/node_modules/electron/dist
    [ -f "$d/electron.bin" ] || mv "$d/electron" "$d/electron.bin"
    printf '#!/bin/sh\nexec "$(dirname "$0")/electron.bin" --no-sandbox "$@"\n' > "$d/electron"
    chmod 755 "$d/electron"
  fi
done
echo PROVISION_USER_OK
