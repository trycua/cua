# test fixture CLI smoke runners

Per-OS broad PASS / FAIL / SKIP probes across the full cua-driver tool
surface. Complements the Rust integration tests under `libs/cua-driver/rust/crates/cua-driver/tests/`:

- Integration tests drive cua-driver through the MCP stdio loop and
  assert on specific UIA / AX state changes.
- Smoke runners drive cua-driver through the CLI (`cua-driver call
  <tool> <json>`) — different code path (CLI argument parser + tool
  resolution), one broad sweep across every registered tool in ~30 sec.

| File              | Host OS  | What it runs                                                   |
|-------------------|----------|----------------------------------------------------------------|
| `macos.sh`        | macOS    | Spawns the AppKit harness, calls every tool once               |

Each script checks in a baseline result file under `results/` for the
current driver version — diff against it to catch regressions in tool
coverage:

```bash
./macos.sh > /tmp/run.txt
diff results/macos.txt /tmp/run.txt
```

Prereqs (macOS):
- `../build/macos.sh` must have produced `libs/cua-driver/rust/test-apps/harness-appkit/`
- `libs/cua-driver/rust/target/release/cua-driver` built (debug works too)
- TCC Accessibility granted to the cua-driver binary
