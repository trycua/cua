#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
exec python3 "$repo_root/cyclops-cs/scripts/check_sdk_binding_contract_sources.py" "$@"
