#!/bin/sh
# Run once the Swift source is in, as the session user: build BenchSentinel.app next to it.
set -e
./build_app.sh BenchSentinel ai.cua.benchsentinel "Bench Sentinel" build
test -x build/BenchSentinel.app/Contents/MacOS/BenchSentinel
