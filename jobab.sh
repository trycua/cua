#!/bin/bash
# Temporary (not committed): tests, the Linux real-harness matrix, then bindings/wasm/docs.
cd libs/cua
cargo test --no-fail-fast -p cua-agent-setup 2>&1 | grep -E "^test .*FAILED|panicked|test result|left:|right:" | head -40
cargo test -p cua-spaces-app-core --lib connection 2>&1 | grep -E "FAILED|panicked|test result" | head -5
cargo build -q -p cua-cli 2>&1 | tail -3
B=$(cargo metadata --format-version 1 --no-deps | python3 -c "import json,sys;print(json.load(sys.stdin)['target_directory'])")
which node npm python3 curl
timeout 2400 python3 ../../real-matrix.py $B/debug/cua > real.json 2> real.err; echo matrix-exit $?; tail -5 real.err; wc -c real.json
cd ../..
bash gen.sh
