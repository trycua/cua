#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Runs flaky.py with whichever Python the runner has (python3 on Linux and
# macOS, python on some Windows images).
set -euo pipefail
py=$(command -v python3 || command -v python) || { echo "no python on this runner" >&2; exit 2; }
exec "$py" "$(dirname "${BASH_SOURCE[0]}")/flaky.py" "$@"
