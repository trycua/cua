`cua-driver permissions` exists on macOS only; Windows and Linux have no TCC equivalent.

- `permissions status` is read-only and never prompts. It does not run the ScreenCaptureKit probe (macOS Tahoe can show a separate dialog for it), so it reports `screen_recording_capturable: null` and `direct_capture_status: "not_checked"`, plus the last successful `permissions grant` verification when one is recorded for the running driver identity.
- `permissions status` reports the running daemon's own answer, which is what its tool calls get. `source.attribution` says whose grants those are: `driver-daemon` when macOS holds the daemon responsible for itself (started through LaunchServices), `caller` when another app is responsible because the daemon was started inside its process tree (then `source.responsible_bundle_id` / `source.responsible_executable` name that app), and `host` for an embedded driver.
- `permissions grant` launches the installed `CuaDriver.app` through LaunchServices so the prompts attribute to the app, then requires a successful live capture probe. It never sends a prompt-capable request over the daemon socket.
- Embedded hosts, in-process `CuaDriver.create()` runtimes and `cua-driver mcp --direct` own their own prompts: there, `check_permissions` stays read-only even with `{"prompt": true}`.
- After a grant changes, quit and relaunch the responsible app (`CuaDriver.app`, or the host of a direct runtime).

See [Permissions](/cua-driver/guides/permissions) for the grant flow and recovery.
