# Cua Agent View Tauri companion

This directory contains the cross-platform presentation process for Cua
Driver's optional Agent View. Cua Driver remains responsible for capture,
window and browser targeting, accessibility, permissions, and input delivery.
The companion renders presentation state only.

The processes communicate through a private newline-delimited JSON protocol on
stdin/stdout. `cua-agent-view` is not a standalone user-facing command and
refuses to start unless Cua Driver passes `--stdio`.

For a source build:

```console
cd libs/cua-driver/agent-view-tauri/src-tauri
cargo build
CUA_AGENT_VIEW_BINARY="$PWD/target/debug/cua-agent-view" \
  cargo run --manifest-path ../../rust/Cargo.toml -p cua-driver -- \
  --agent-view serve
```

Installed packages place `cua-agent-view` beside the Cua Driver executable, so
the environment override is unnecessary. Until all distribution paths include
the companion, Cua Driver falls back to the existing native Agent View.
