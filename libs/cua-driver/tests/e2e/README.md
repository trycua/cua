# cua-driver Python e2e tests

Python e2e tests drive the public MCP tool interface against real apps and
shared harness apps. They are manual/VM-backed tests, not default CI tests.

## Layout

```text
tests/e2e/
├── driver_client.py      # raw MCP client
├── conftest.py           # pytest fixtures
├── harness/              # typed wrappers, tree helpers, CV helpers, UX monitor
├── assets/               # symlinks/references for browser and Blender tests
├── PHILOSOPHY.md         # black-box test pattern and UX invariants
├── run_tests.sh          # pytest runner that builds cua-driver if needed
└── test_*.py             # app and legacy e2e modules
```

## Running

```bash
cd libs/cua-driver/tests/e2e
./run_tests.sh test_electron -v
python3 -m pytest test_tauri.py -v
```

Set `CUA_DRIVER_BINARY` to test a specific binary. Without it, `run_tests.sh`
builds `libs/cua-driver/rust` and uses `rust/target/debug/cua-driver`.

## Harness Apps

`test_electron.py` and `test_tauri.py` use repo-local shared harnesses:

- `tests/fixtures/apps/cross-platform/electron`
- `tests/fixtures/apps/cross-platform/tauri`

The fixtures build missing staged apps on demand with each harness `build.sh`.
They no longer download prebuilt app releases.

## Rules

- Assert through observable state in the AX/UIA/AT-SPI tree or screenshots.
- Do not call internal browser APIs from tests.
- Keep app-specific setup in fixtures, not in driver code.
- Treat older root-level tests as legacy coverage until they are audited and
  either ported into the shared harness pattern or removed.
