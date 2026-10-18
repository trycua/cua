#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# `swift test` for this package; also works with only the Command Line Tools
# installed (swift-testing lives outside the default search paths there).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
clt=/Library/Developer/CommandLineTools/Library/Developer
extra=()
if [[ "$(uname)" == Darwin ]] && ! xcodebuild -version >/dev/null 2>&1 && [[ -d $clt/Frameworks ]]; then
  extra=(-Xswiftc -F -Xswiftc "$clt/Frameworks" -Xlinker -F -Xlinker "$clt/Frameworks"
    -Xlinker -rpath -Xlinker "$clt/Frameworks" -Xlinker -rpath -Xlinker "$clt/usr/lib")
fi
# Recompile everything when the cua SDK binding or the Cua Spaces app
# binding changed (see fresh-abi.sh).
app_binding=../../libs/spaces-app-swift/Sources
export CUA_SWIFT_ABI_EXTRA="$app_binding/CuaSpacesFFI/CuaSpacesFFI.swift $app_binding/cua_spaces_ffiFFI/include/cua_spaces_ffiFFI.h"
../../libs/cua/swift/scripts/fresh-abi.sh . debug
# ${extra[@]+...}: bash 3.2 (macOS) treats an empty array as unset under set -u.
exec swift test ${extra[@]+"${extra[@]}"} "$@"
