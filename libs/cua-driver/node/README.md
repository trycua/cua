# `@trycua/cua-driver-embedded`

Host a private cua-driver process inside a Node or Electron application. The host bundles the `cua-driver` executable, starts it directly, and owns its lifetime. On macOS, that direct process relationship lets the driver use the signed host application's Accessibility and Screen Recording grants.

This package does not install cua-driver and does not start a global daemon.

```ts
import { EmbeddedCuaDriver } from '@trycua/cua-driver-embedded';

const driver = new EmbeddedCuaDriver({
  binaryPath: '/path/inside/YourApp.app/cua-driver',
  hostBundleId: 'com.example.your-app',
});

const connection = await driver.start();
// Configure your agent's MCP server with connection.mcp.

await driver.stop();
```

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

Call `driver.restart()` after a permission change if the child was already running. macOS caches permission answers per process.
