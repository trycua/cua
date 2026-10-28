#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Structure check for the Windows NSIS installers, in Docker (no Windows
# needed). Unpacks each installer with 7-Zip and checks the payload, the PE
# headers and version resources, the asar, and the signature state. It does
# not install or run anything; see the README for what that leaves untested.
#
#   scripts/smoke/windows.sh
#
# Report: dist/smoke/windows/report.txt.
set -euo pipefail
cd "$(dirname "$0")/../.."
out="$PWD/dist/smoke/windows"
rm -rf "$out" && mkdir -p "$out"
docker build -q -t cua-spaces-smoke:win-tools -f scripts/smoke/windows.Dockerfile scripts/smoke >/dev/null
docker run --rm -e VERSION="$(node -p 'require("./package.json").version')" \
  -v "$PWD/dist:/dist:ro" -v "$out:/out" -v "$PWD/scripts/smoke/windows-inside.py:/check.py:ro" \
  cua-spaces-smoke:win-tools python3 /check.py 2>&1 | tee "$out/report.txt"
