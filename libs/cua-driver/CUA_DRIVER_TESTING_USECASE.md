# cua-driver UI Testing Framework

Implementation plan and reference for the Playwright-like UI testing layer on top of cua-driver.

## Implementation location

```
libs/cua-driver/sdk/
├── python/                           # Python SDK + pytest plugin
│   ├── src/
│   │   ├── cua_driver/
│   │   │   ├── _client.py            # DriverClient (sync MCP stdio)
│   │   │   ├── _app.py               # App
│   │   │   ├── _window.py            # Window
│   │   │   ├── _locator.py           # Locator (auto-waiting)
│   │   │   ├── _expect.py            # expect() sync assertions
│   │   │   ├── _async.py             # AsyncDriverClient, AsyncApp/Window/Locator, async_expect
│   │   │   └── __init__.py
│   │   └── pytest_cua/
│   │       └── plugin.py             # pytest fixtures: cua_driver, cua_app, async_cua_driver, async_cua_app
│   └── pyproject.toml
├── typescript/                       # TypeScript SDK
│   ├── src/
│   │   ├── client.ts                 # DriverClient (async, Symbol.asyncDispose)
│   │   ├── app.ts                    # App
│   │   ├── window.ts                 # Window
│   │   ├── locator.ts                # Locator (auto-waiting)
│   │   ├── expect.ts                 # expect() async assertions
│   │   └── index.ts
│   ├── package.json
│   └── tsconfig.json
├── codegen/
│   └── generate.py                   # Generates typed tool wrappers from live MCP schema
└── examples/
    ├── python/
    │   ├── pytest.ini                 # -n auto --dist loadfile --asyncio-mode=auto
    │   ├── conftest.py                # sys.path setup for uninstalled SDK
    │   ├── test_calc_addition.py      # sync, xdist worker 1
    │   ├── test_calc_multiplication.py # sync, xdist worker 2
    │   ├── test_calc_async.py         # async (pytest-asyncio), xdist worker 3
    │   ├── test_calculator.py         # original examples (standalone)
    │   └── test_safari_form.py        # Safari form fill
    └── typescript/
        ├── vitest.config.ts           # pool: forks, fileParallelism: false
        ├── calculator.test.ts         # addition + multiplication (merged)
        ├── safari-form.test.ts        # Safari form fill
        ├── calculator-addition.test.ts    # standalone example (not in default run)
        └── calculator-multiplication.test.ts  # standalone example (not in default run)
```

## Architecture summary

```
cua-driver daemon (Swift, MCP JSON-RPC 2.0 over stdio)
    │
    ├── Python: cua.driver.DriverClient   (mcp PyPI SDK)
    └── TypeScript: @cua/driver           (mcp npm SDK)
            │
            ├── App / Window / Locator / expect()
            └── pytest-cua / @cua/vitest fixtures
```

## Key protocol facts (needed for implementation)

- `list_apps` → text only (no structuredContent). Parse text lines: `- AppName (pid N) [bundle.id]`
- `list_windows` → structuredContent.windows array: `{window_id, pid, app_name, title, bounds, is_on_screen, on_current_space}`
- `get_window_state` → image content block + text block (treeMarkdown). NO structuredContent.
  - AX tree format: `INDENT- [N] AXRole "Title" [value="..."] [actions=[...]]`
  - `[N]` is the element_index passed to click/set_value/etc.
  - Indent is 2 spaces per depth level
- `click` → requires `pid` + either `element_index`+`window_id` OR `x`+`y`
- `launch_app` → returns pid + windows array in structuredContent
- AX element click path: get_window_state → parse tree → find element index → click(pid, window_id, element_index)

## Phases

1. **Low-level clients** — async DriverClient in Python + TypeScript (MCP stdio wrappers)
2. **Codegen** — `generate.py` introspects live MCP server, emits typed tool bindings for both languages
3. **High-level API** — App, Window, Locator, expect() with auto-waiting
4. **Auto-waiting** — Locator polls get_window_state + re-parses tree until element found or timeout
5. **Test runner plugins** — pytest-cua fixtures, @cua/vitest fixtures
6. **Example test suites** — Calculator arithmetic, Safari form fill

## Status

- [x] Phase 1: Python DriverClient — `sdk/python/src/cua_driver/_client.py`
- [x] Phase 1: TypeScript DriverClient — `sdk/typescript/src/client.ts` (async, Symbol.asyncDispose)
- [x] Phase 1: Python AsyncDriverClient — `sdk/python/src/cua_driver/_async.py`
- [x] Phase 2: Codegen script — `sdk/codegen/generate.py`
- [x] Phase 3: Python App/Window/Locator/expect — `sdk/python/src/cua_driver/`
- [x] Phase 3: Python Async layer — `AsyncApp/AsyncWindow/AsyncLocator/async_expect` in `_async.py`
- [x] Phase 3: TypeScript App/Window/Locator/expect — `sdk/typescript/src/`
- [x] Phase 4: Auto-waiting — Python `Locator._wait_for_element()`, TypeScript `Locator.waitForElement()`
- [x] Phase 5: pytest-cua plugin — sync + async fixtures with xdist support
- [x] Phase 5: Vitest config — `pool: forks, fileParallelism: false` for per-file process isolation
- [x] Phase 5: `kill_app` convenience helper — Python `kill_app(bundle_id, force=False)` + TypeScript `killApp(bundleId, force=false)`
- [x] Phase 6: Python examples — 21 tests across 4 files (calc addition/multiplication/async + safari form)
- [x] Phase 6: TypeScript examples — 17 tests across 2 files (calculator.test.ts + safari-form.test.ts)

### Key bugs fixed during implementation

- `get_window_state` returns tree in `content[N]["text"]` (NOT `structuredContent`)
- Multi-word queries (e.g. `"AXButton 2"`) return filtered-empty trees; only role should be passed
- Digit label `"2"` matched `[2]` indices — removed loose substring fallback from `matchesLine`
- After pressing Equals, Calculator has TWO AXStaticText elements; assertions must check ALL, not first
- Safari AX tree uses label text (e.g. `"Text"`, `"Email"`) not HTML element IDs (`f-text`) for matching
- `asyncio.to_thread`-based `AsyncApp.launch` is a coroutine — use `app = await AsyncApp.launch(...)` not `async with`
- `close()` in TypeScript DriverClient now escalates to SIGKILL after 1 s to prevent orphaned processes

## Running the example test suites

### Python (sequential, pytest)

```bash
cd libs/cua-driver

# Install deps (one-time)
pip install pytest pytest-asyncio
# or: uv run --with pytest --with pytest-asyncio python -m pytest ...

# Run all example tests
pytest sdk/examples/python/ -v -p no:xdist

# Run a single file
pytest sdk/examples/python/test_calc_addition.py -v
```

> **Note**: parallel execution via `-n auto --dist loadfile` is intentionally
> disabled (see `pytest.ini` comment). Calculator is a macOS single-instance app
> and multiple cua-driver processes conflict on accessibility APIs when run
> simultaneously.

### TypeScript (sequential file execution via Vitest forks)

```bash
cd sdk/examples/typescript   # IMPORTANT: run from this directory

# Install deps (one-time)
npm install

# Run all test files (calculator.test.ts + safari-form.test.ts)
npx vitest run

# Run a single file
npx vitest run calculator.test.ts
npx vitest run safari-form.test.ts
```

> **Note**: `fileParallelism: false` ensures files run sequentially to avoid
> macOS accessibility API contention between multiple cua-driver instances.
> Each file still runs in its own isolated child-process fork.
