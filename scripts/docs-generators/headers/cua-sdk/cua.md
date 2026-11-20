| Topology | Create | Behavior |
| --- | --- | --- |
| Embedded | `Cua.embedded(config)` (Python `cua.embedded()`, TypeScript `embedded()`) | The runtime lives in your process |
| Daemon client | `Cua.connect(address, token)` (Python `cua.connect()`, TypeScript `connect()`) | Calls a running `cua daemon`, which shares sandboxes, spacesd connections, tunnels and credentials across processes |

`connect()` without an address reads `~/.cua/daemon.json`, then the socket `~/.cua/cua.sock`. The connection is lazy: the first call fails with `Transport` when no daemon runs (`cua daemon start`).
