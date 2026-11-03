#!/bin/sh
# Run once the Swift sources are in, as the session user: build BenchLab.app next to them (the Command Line Tools'
# swiftc, as the bench's own build does).
set -e
./build_app.sh BenchLab ai.cua.benchlab BenchLab build
test -x build/BenchLab.app/Contents/MacOS/BenchLab
