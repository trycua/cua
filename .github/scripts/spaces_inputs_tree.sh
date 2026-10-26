#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Prints one hash of everything that can change the shipped bytes of Cua
# Spaces at REV (default HEAD): the git trees of the release inputs. The same
# hash means the same build. Used by cd-cua-spaces-build.yml (a live build of
# the tree is not built again) and nightly-cua-spaces.yml (no nightly when
# nothing changed since the last one). Keep the list in step with
# cd-cua-spaces-build.yml's `on.push.paths`.
#
#   .github/scripts/spaces_inputs_tree.sh [REV]
set -euo pipefail
rev="${1:-HEAD}"
inputs=(
  apps/cua-spaces-macos apps/cua-spaces apps/cua-spaces-desktop apps/cua-spaces-web
  libs/cua libs/cua-spacesd/VERSION
  .github/actions/spaces-native .github/actions/spaces-rust-slice .github/actions/setup-protoc
  .github/workflows/cd-cua-spaces-build.yml
)
args=()
for p in "${inputs[@]}"; do args+=("$rev:$p"); done
git rev-parse "${args[@]}" | git hash-object --stdin
