# @trycua/computer

Computer-Use Interface (CUI) framework for interacting with local desktops, sandboxes, and cloud VMs.

**[Documentation](https://cua.ai/docs/cua/reference/computer-sdk)** - Installation, guides, and configuration.

## Usage

### Connect to the host desktop

Drive the desktop of the machine the SDK runs on through a local
[Computer Server](https://cua.ai/docs/libraries/computer-server). No VM or API
key required — mirrors the Python SDK's `use_host_computer_server=True`.

```ts
import { Computer } from '@trycua/computer';

const computer = new Computer({ useHostComputerServer: true });
await computer.run(); // Connect to the host desktop

const screenshot = await computer.interface.screenshot();

await computer.disconnect();
```

`osType` defaults to the OS the process is running on. You can point at a
non-default host/port (for example a Computer Server reachable over the network):

```ts
const computer = new Computer({
  useHostComputerServer: true,
  host: '192.168.1.50',
  port: 8000,
});
```

`HostComputer` is an explicitly-typed alias for the same thing:

```ts
import { HostComputer } from '@trycua/computer';

const computer = new HostComputer();
await computer.run();
```

### Connect to a cloud VM

Provide a Cua API key and the VM name:

```ts
import { Computer, OSType } from '@trycua/computer';

const computer = new Computer({
  apiKey: process.env.CUA_API_KEY!,
  name: process.env.CUA_CONTAINER_NAME!,
  osType: OSType.LINUX,
});
await computer.run();
```
