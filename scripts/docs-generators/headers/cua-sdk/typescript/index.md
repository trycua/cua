`npm install @trycua/cua` ({{version}}) installs the Node package; native code ships as per-platform optional packages (`@trycua/cua-darwin-arm64`, `@trycua/cua-linux-x64-gnu`, ...).

| Entry point | Use it for |
| --- | --- |
| `@trycua/cua` | `embedded()`, `connect()`, every generated object, and the helpers on this page |
| `@trycua/cua/spaces`, `/spaces/transport`, `/spaces/host` | Spaces threads, approvals and transports |
| `@trycua/cua/browser` | A WebAssembly subset: cua-spacesd over gRPC-Web and a Fleet client with a static bearer. No sandboxes, local runtimes, Spaces, daemon, file transfer or media. Give it a short-lived token from your backend, never client secrets |

cua does not implement MCP: `connectMcp(await sb.mcpConfig(service, undefined))` hands the endpoint to the official SDK (`npm install @modelcontextprotocol/client`). Cloud bearers are short-lived, so fetch a config per connection.
