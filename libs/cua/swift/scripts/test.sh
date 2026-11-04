#!/usr/bin/env bash
# `swift test` that also works with only the Command Line Tools installed
# (swift-testing lives outside the default search paths there).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
clt=/Library/Developer/CommandLineTools/Library/Developer
extra=()
if [[ "$(uname)" == Darwin ]] && ! xcodebuild -version >/dev/null 2>&1 && [[ -d $clt/Frameworks ]]; then
  extra=(-Xswiftc -F -Xswiftc "$clt/Frameworks" -Xlinker -F -Xlinker "$clt/Frameworks"
    -Xlinker -rpath -Xlinker "$clt/Frameworks" -Xlinker -rpath -Xlinker "$clt/usr/lib")
fi
# Recompile everything when the generated binding changed (see fresh-abi.sh).
scripts/fresh-abi.sh . debug
# ${extra[@]+...}: bash 3.2 (macOS) treats an empty array as unset under set -u.
exec swift test ${extra[@]+"${extra[@]}"} "$@"
