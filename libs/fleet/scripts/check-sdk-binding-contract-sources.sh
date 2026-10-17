#!/usr/bin/env bash
set -euo pipefail

workspace_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec python3 "$workspace_dir/scripts/check_sdk_binding_contract_sources.py" "$@"
