#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# `swift test` that also works with only the Command Line Tools installed
# (swift-testing lives outside the default search paths there). Stage the
# library first: ../../libs/spaces-app-swift/scripts/stage-library.sh (the Cua
# Spaces app export, which carries the cua SDK too).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
clt=/Library/Developer/CommandLineTools/Library/Developer
extra=()
if [[ "$(uname)" == Darwin ]] && ! xcodebuild -version >/dev/null 2>&1 && [[ -d $clt/Frameworks ]]; then
  extra=(-Xswiftc -F -Xswiftc "$clt/Frameworks" -Xlinker -F -Xlinker "$clt/Frameworks"
    -Xlinker -rpath -Xlinker "$clt/Frameworks" -Xlinker -rpath -Xlinker "$clt/usr/lib")
fi
# ${extra[@]+...}: bash 3.2 (macOS) treats an empty array as unset under set -u.
exec swift test ${extra[@]+"${extra[@]}"} "$@"
