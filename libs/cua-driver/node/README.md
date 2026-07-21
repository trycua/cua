# `@trycua/cua-driver-embedded`

Host a private cua-driver process inside a Node or Electron application. The host bundles the `cua-driver` executable, starts it directly, and owns its lifetime. On macOS, that direct process relationship lets the driver use the signed host application's Accessibility and Screen Recording grants.

This package does not install or bundle cua-driver and does not start a global daemon. Ship a compatible executable with the host application and pass its absolute path as `binaryPath`.

```ts
import { EmbeddedCuaDriver } from '@trycua/cua-driver-embedded';

const driver = new EmbeddedCuaDriver({
  binaryPath: '/path/inside/YourApp.app/cua-driver',
  hostBundleId: 'com.example.your-app',
});

try {
  const connection = await driver.start();
  // Configure your agent's MCP server with connection.mcp.
} finally {
  await driver.stop();
}
```

`stop()` first sends the daemon `SIGTERM`, then force-kills it after `shutdownTimeoutMs`. The package cannot await asynchronous cleanup after its host has already exited, so call `stop()` from every orderly shutdown path.

Cua Driver defaults to its protected `standard` permission mode. A trusted host can select `bounded` or `unrestricted` through the `environment` option, but must never expose those values to the agent. Unrestricted mode requires both `CUA_DRIVER_PERMISSION_MODE=unrestricted` and `CUA_DRIVER_DANGEROUSLY_BYPASS_APPROVALS=1`.

Electron hosts can request the grants from their main process after `app.whenReady()`:

```ts
import {
  hasRequiredMacOSPermissions,
  openMacOSScreenRecordingSettings,
  requestMacOSPermissions,
} from '@trycua/cua-driver-embedded/electron';

const permissions = requestMacOSPermissions();
if (!hasRequiredMacOSPermissions(permissions)) {
  if (!permissions.screenRecording) await openMacOSScreenRecordingSettings();
  // Do not start the driver yet. Ask the user to grant access and restart.
}
```

Stop the child before Electron exits. `before-quit` must defer the first quit request while cleanup completes:

```ts
let driverStopped = false;
app.on('before-quit', (event) => {
  if (driverStopped) return;
  event.preventDefault();
  void driver.stop().finally(() => {
    driverStopped = true;
    app.quit();
  });
});
```

Call `driver.restart()` after a permission change if the child was already running. macOS caches permission answers per process.

## Packaging on macOS

Bundle a universal or architecture-matched `cua-driver` executable outside Electron's ASAR archive, preserve its executable bit, and resolve `binaryPath` from the packaged application rather than the development checkout. Sign the nested executable before signing the enclosing app, then verify the final bundle with your normal `codesign --verify --deep --strict` and notarization checks.

The Electron permission adapter uses the native `ffi-rs` module. Keep its `.node` binary outside ASAR as well; Electron Forge and electron-builder can unpack native modules, but verify the packaged app on both Apple Silicon and Intel when shipping a universal build.

The app's main process must spawn the daemon directly. Starting it through a gateway, terminal, `open`, or `NSWorkspace` changes the macOS responsibility chain, so the daemon will not inherit the app's grants.
