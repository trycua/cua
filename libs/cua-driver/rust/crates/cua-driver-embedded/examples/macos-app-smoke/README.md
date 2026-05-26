# macOS app smoke test

Builds a temporary signed `.app` that links `libcua_driver_embedded.dylib`,
launches through LaunchServices, and calls the embedded MCP API internally.

The app intentionally calls only:

- `initialize`
- `tools/list`
- `check_permissions` with `{"prompt": false}`

That verifies bundle attribution without opening permission prompts.

```bash
./crates/cua-driver-embedded/examples/macos-app-smoke/run.sh
```
