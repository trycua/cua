# @trycua/computer

Computer-use interface for controlling local, legacy Cloud, and Fleet-backed
macOS, Linux, and Windows sandboxes.

## Fleet

`FleetComputer` claims a sandbox from an existing Fleet pool, connects to its
computer-server service through the authenticated Fleet WebSocket proxy, and
releases the claim on `stop()` or `disconnect()`.

```ts
import { FleetComputer, OSType } from '@trycua/computer';

const computer = new FleetComputer({
  name: 'keiki-task',
  osType: OSType.LINUX,
  accessToken: process.env.CUA_FLEET_ACCESS_TOKEN!,
  poolName: 'keiki-pool',
  namespace: 'keiki',
  serviceName: 'server',
});

await computer.run();
try {
  const screenshot = await computer.interface.screenshot();
  console.log(screenshot.length);
} finally {
  await computer.stop();
}
```

The package depends on `@trycua/fleet/node`; it does not vendor a second copy of
the Fleet lifecycle implementation.

**[Documentation](https://cua.ai/docs/cua/reference/computer-sdk)** - Installation, guides, and configuration.
