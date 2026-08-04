# Cua Driver TypeScript MCP server

This private package hosts Cua Driver through the official Model Context
Protocol TypeScript SDK. It uses the generated TypeScript UniFFI package and
the release-matched skill pack in `../rust/Skills/cua-driver`. It is not
published and does not alter a release workflow.

## Execution boundary

The generated binding exposes the two open-ended methods a protocol adapter
needs: `CuaDriver.listToolsJson()` returns the canonical platform inventory,
and `CuaDriver.callTool()` returns a `ToolResult.rawJson` copy of the MCP result.
The server therefore preserves dynamic input and output schemas, annotations,
text and image content, structured content, and tool errors without rebuilding
every platform tool in TypeScript.

The server connects with `CuaDriver.connect()`. It does not normally call
`CuaDriver.create()`, because a same-process runtime would move desktop
ownership into Node and, on macOS, change the TCC responsibility identity. The
connection accepts both standalone and app-owned embedded daemons; the daemon
process remains the execution boundary in either case. One generated client
remains connected to one daemon socket for the MCP server's lifetime. Public session names, daemon state, consent decisions, desktop
identity, and TCC attribution remain at the released daemon boundary. Destroying
the client does not stop a daemon that it does not own.

The generated JavaScript API does not expose three Rust-only adapter methods:
`CuaDriver::inspect_host_tools`, `CuaDriver::connect_remote`, and
`CuaDriver::call_tool_from_trusted_adapter`. Rust-only `DriverHostOptions` also
contains `register_host_tools`, a function-pointer hook that UniFFI cannot
represent. JavaScript therefore cannot register daemon administrative tools,
provide an authenticated remote carrier, or inject the Rust MCP proxy's private
transport-session evidence. The public generic call sanitizes reserved
arguments. This package does not simulate those capabilities.

The generated API currently has 19 typed desktop methods and four typed session
methods. Those 23 methods are a portable typed subset. The generic inventory and
call methods expose the full live platform registry through the daemon. The
server uses the SDK's low-level `Server` API so it can forward the live JSON
Schema and annotations without translating them through another schema system.

## Skill pack

Initialization instructions contain a compact `<available_skills>` catalog.
The read-only `load-skill` tool loads `SKILL.md` on demand and may load a
referenced Markdown companion such as `BROWSER.md`. Companion files are also
read-only MCP resources. Paths use `realpath`, remain inside the bundled root,
are restricted to Markdown, and reject absolute paths, traversal, and symlink
escapes. Mutable local, GitHub, archive, and arbitrary remote skill sources are
not enabled.

## Run

Build the sibling `../typescript` package and stage its release-matched native
library. Start the installed Cua Driver daemon with the intended policy and TCC
identity, then run:

```bash
npm install
npm run build
CUA_DRIVER_SOCKET=/path/to/cua-driver.sock node dist/cli.js
```

`CUA_DRIVER_TYPESCRIPT_MODULE` may point to another built copy of the same
release-matched generated package for local validation.

## Costs and limitations

- Startup performs one daemon metadata check and one inventory request. Each
  tool call adds a Node-to-UniFFI-to-local-socket hop.
- The inventory is cached for the server lifetime. Restart after a daemon
  generation or tool-set change; old generation connections are not reusable.
- Cancellation and MCP transport-session evidence are not exported by the
  public generic UniFFI call. Daemon tool and session policy still applies, but
  this adapter cannot claim the Rust proxy's private trusted-evidence path.
- Tests validate protocol and adapter contracts without a native desktop. Native
  execution and macOS/Windows permission identity require release-matched
  artifacts on those operating systems.
- Skilljack's mutable discovery, watching, prompts, UI, subscriptions, and
  remote-source features remain out of scope.
