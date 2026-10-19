# cua-spaces

The Spaces core in Rust: a registry of sandboxes that run cua-spacesd,
the primitives a person or agent uses on them, and the Spaces MCP server.
Space ids are sandbox refs: `cloud:<name>`, `local:<vm>`,
`direct:<host:port>` and `relay:<machine-id>` (legacy `space://...` ids still
parse).
Everything goes through the cua SDK crates (`cua-spacesd-client`, `cua-fleet`,
`cua-sandbox-core`): no ssh, no `/cmd`, no base64 through a shell, no
Python.

Plain async Rust, no UniFFI. `cua-daemon` hosts it, `cua-sdk` wraps it
for bindings, and the Tauri app links it directly.

## API (for `cua-daemon`, `cua-sdk` and the Tauri app)

```rust
use cua_spaces::{Spaces, FleetClaim, LocalProvision};

let spaces = Spaces::builder()
    .fleet(cua_fleet::FleetClient::from_env()?)          // optional: cloud Spaces
    .local_runtime(daemon_local_runtime)                  // optional: Arc<dyn cua_sandbox_core::LocalRuntime>
    .app_sessions(Arc::new(cua_spaces::teleport::AppSessions::builtin()))   // teleport sender (real host)
    .agent_credentials(cua_spaces::agents::HostCredentials::from_home(home)) // host creds copied for agents
    .operator_display(app_display)                        // default: the app's loopback control server
    .relay(relay_account)                                 // optional: relay: machines of this account
    .build();                                             // registry: $CUA_HOME or ~/.cua
```

| Call | What |
|---|---|
| `spaces.add(url, token, name)` | `GetCapabilities` handshake, then stored. `url` is `http(s)://host:port`, `host:port`, or a Space id (`local:<name>`, `cloud:<name>`). |
| `spaces.list()` / `spaces.resolve(s)` / `spaces.remove(s)` | Registry. `resolve` accepts ids, legacy `space://...` / `fleet:ns:claim`, URLs, names; a name in two locations is `AmbiguousSandbox`. |
| `spaces.create(SpaceCreate{image, on, kind, runtime, name, reuse, wait, ..})` | A new sandbox registered as a Space, where `on` says (`None`: `default.on`, `CUA_DEFAULT_ON`, else local). Placement is validated once (`InvalidPlacement` lists the valid values); cloud Spaces get warm capacity per (image, runtime) and a fresh env token. `reuse` returns a reachable registered Space in the same location first. |
| `spaces.delete(s)` | Deletes the Space's sandbox and forgets it. A Space added by address is only forgotten. |
| `spaces.space(s) -> Space` | A cached, connected handle. |
| `spaces.hotspot_start / hotspot_stop / hotspot_statuses` | Hotspots kept alive by this process. |
| `spaces.relay_machines()` | The signed-in account's machines on the relay (`cua host setup` registers them), each a `relay:<id>` Space. |

A `Space` checks the spacesd feature each primitive needs
(`space.require("window_stream")`) and fails with
`Error::CapabilityMissing { feature, limitation }` otherwise. `Error::tag()`
is the stable machine kind (`capability_missing`, `spacesd_not_available`,
`host_capability_missing`, `teleport_refused`, ...).

| Module | `Space` calls |
|---|---|
| `exec` | `bash(cmd, timeout) -> BashOutput` (`.render()` is the `space_bash` text), `write(path, bytes)`, `home()` |
| `files` | `upload(local, dest)`, `download(remote, dest_dir)`, `send_file(local, SendFileOptions{subdir, respect_ignore_files, conflict})` through `TeleportService.ReceiveFiles`, per-file sha256 |
| `services` | `list_tools(service)`, `call_tool(service, tool, args, timeout) -> ToolResult` (MCP content parts) |
| `stream` | `windows(app)`, `find_window(app)`, `stream_targets`, `open_stream(target, opts) -> StreamTicket` (ticket + `ws_url`), `close_stream`, `stream_session(target, opts, Arc<dyn FrameSink>, Option<Arc<dyn AudioSink>>) -> StreamSession` |
| `presence` | `join_presence(Identity, timeout) -> PresenceSession` (`roster`, `next_event`, `wait_for`, `update_cursor`, `leave`) |
| `teleport` | `AppSessions::manifest(app, scope) -> TeleportManifest`, `manifest.approving(space, paths, ack)` / `approving_default(space, ack) -> Approval`, `space.teleport(sessions, &approval, ImportOptions) -> TeleportReceipt`, `teleport_receiver(app, scope)` |
| `hotspot` | `start_hotspot(HotspotOptions{socks_port, set_system_proxy, bypass, dialer}) -> Hotspot`, `hotspot_status()` |
| `agents` | `space.agents(HostCredentials) -> Agents`: `start(agent, prompt, StartOptions)`, `status(run, tail)`, `message(run, text, force)`, `stop(run)`, `list()`; `agents::capabilities()` |

`StreamSession` delivers encoded frames (keyframe-gated per codec epoch by
`cua-media-client`'s v2 state machine) and audio packets on one
delivery thread; decoding stays with the consumer. A slow sink drops video
and forces a keyframe resync; audio and events are never dropped before the
256-item bound. `stats()`, `request_keyframe()`, `send_input(events)`,
`close()`.

Teleport send uses the SDK's `cua-teleport` providers, confined to
`src/teleport.rs` (`teleport::providers` re-exports what callers need). The
receiving side is cua-spacesd's `TeleportService`.

### MCP (`cua daemon mcp`, `cua mcp`, daemon `/mcp`)

```rust
let server = cua_spaces::mcp::McpServer::new(spaces.clone());
cua_spaces::mcp::stdio::serve_process_stdio(server.clone()).await?;          // stdio
cua_spaces::mcp::http::serve(server, listener, Some(bearer)).await?;          // streamable HTTP at /mcp
let response: Option<serde_json::Value> = server.handle(json_rpc).await;      // transport-free

// Forward the contract to another process and add non-contract tools:
let server = McpServer::remote(Arc::new(my_backend))      // impl ToolBackend
    .with_extension(Arc::new(my_tools))                     // impl ToolExtension
    .with_filter(Arc::new(|tool| allowed(tool)));
// Run a Space-scoped handler on an already-connected Space:
let out = cua_spaces::mcp::call_on(&space, "space_bash", json!({"command": "ls"})).await;
```

`tools/list` is `cua_spaces_contract::tools()` (33 tools, schemars input
schemas); arguments deserialize into the same `cua_spaces_contract::inputs`
types. `cua_spaces::client` is the typed control-plane client over any MCP
transport (`ScriptedTransport`, `InProcessTransport`).

Where it is served (one implementation of every contract tool):

| Endpoint | Server | Contract tools run in |
|---|---|---|
| `cua daemon mcp` (stdio; starts the daemon if none runs) | `McpServer::remote` + the CLI extension, `serverInfo.name = "cua"` | the daemon (`SpaceService.CallSpaceTool`) |
| `cua mcp` / `serve-mcp` (stdio) | same | the daemon when one runs, else embedded (`cua_sdk::Spaces::call_tool_json`) |
| daemon loopback `POST /mcp` (streamable HTTP, daemon bearer) | `McpServer::new(runtime.spaces())`, `serverInfo.name = "cua-spaces"` | the daemon |

#### `cua mcp`'s sandbox tools vs the Spaces contract

The `cua` CLI's sandbox/computer/skills tools are an extension of the same
server (permissions `sandbox:*`, `computer:*`, `skills:*`; the contract is
`spaces:all`, `spaces:readonly` or `spaces:<tool>`). Overlaps, reconciled:

| `cua mcp` tool | Contract tool | Resolution |
|---|---|---|
| `computer_shell` | `space_bash` | **One handler**: runs `space_bash` on the sandbox's spacesd (`mcp::call_on`). Output is now the `space_bash` text (`stdout`, `[stderr]`, `[exit N]`) instead of `{stdout, stderr, returncode}`; timeout 120 s. |
| `computer_file_write` | `space_write` | **One handler**: `space_write` (parents created, SHA-256 verified); replies `wrote <path> (<n> bytes, sha256 …)`. |
| `computer_file_read` | `download` | Kept: returns the file in the reply; `download` writes to a host directory. |
| `computer_window_list` | `list_space_windows` | Kept: window management (`WindowsService`) vs streamable targets. |
| `sandbox_list` / `sandbox_create` / `sandbox_delete` | `list_spaces` / `create_space` / `delete_space` | Kept: sandboxes are daemon-agnostic (may run no spacesd, `~/.cua/sandboxes`); a Space needs a spacesd (`~/.cua/spaces.json`). |
| other `computer_*`, `skills_*` | none | CLI-only (pixel-space computer use, recorded skills). |
| none | `add_space`, `stream_*`, `teleport_*`, `hotspot_*`, `agent_*`, pip/viewer | Spaces-only. |

Protocol conventions are the Spaces server's: an unknown or non-permitted
tool is JSON-RPC `-32601` (the CLI used `-32602`), and tool failures carry
`structuredContent.error.kind` (`capability_missing`, `teleport_refused`, ...).

## Registry files

- `~/.cua/spaces.json`: `[cua.daemon.v1.Space]` in proto3 JSON (the shape
  `SpaceService.ListSpaces` serves). No secrets.
- `~/.cua/spaces-credentials.json`: `{id: {url, token}}`, mode 0600.
- `~/.cua/spaces.lock`: advisory lock held across every read-modify-write,
  so the Spaces app, the CLI and `cua daemon` can add Spaces concurrently.

## In `cua daemon` and the SDK

`cua_daemon::Runtime` owns one `Spaces` (sharing its sandboxes and Fleet
client). `cua.daemon.v1.SpaceService` is a thin adapter: `AddSpace`,
`ListSpaces`, `ResolveSpace`, `RemoveSpace`, `ClaimFleetSpace`,
`ProvisionLocalSpace`, `ReleaseSpace`, `ListSpaceTools`, `CallSpaceTool`,
and `ConnectSpace`, which returns a loopback env passthrough
(`/v1/spaces/<base64url id>/env`, daemon bearer only; the Space's env token
and Fleet credentials are attached inside the daemon, WebSockets included).
`OpenMediaBridge` accepts Space ids too. `cua-sdk` exposes `Spaces` /
`Space` to every language in both topologies (see its crate docs).

## Features

All on by default: `spaces-files`, `spaces-stream`, `spaces-presence`,
`spaces-teleport`, `spaces-hotspot`, `spaces-agents`, `mcp`, `mcp-http`,
`mcp-client`. With a module compiled out, its MCP tools answer
`host_capability_missing`.

## Tests

```sh
cargo test -p cua-spaces                     # unit + in-process env-server + fakes + MCP conformance
tests/e2e/run-docker-e2e.sh [--runtime runsc] [--driver PATH]   # live linux, --memory=4g
tests/e2e/run-relay-account-e2e.sh                               # host joins cua-relay, client lists and streams it
CUA_E2E_FLEET=1 CUA_E2E_FLEET_IMAGE=<public spacesd image> cargo test -p cua-spaces --test e2e_fleet
cargo run -p cua-spaces --bin cua-spaces-control-fixtures -- --check
```

The in-process tests run the real `cua-spacesd-server` on loopback with a temp
`$HOME` (set through `SystemService.Init`), temp Downloads and teleport home,
`FakeHost` on both teleport ends and a fake tool registry.

## Migration notes (from the former Python Spaces server)

- Ids are sandbox refs (`local:`, `cloud:`, `direct:`, `relay:`); the old spellings still parse.
- `local_rcdp` is `stream_endpoint` and returns a media ticket for any
  provider (no ssh-read per-boot token).
- `add_space` / `remove_space` are new; hotspot tools are capability-gated
  (`hotspot`) rather than Fleet-only, run in this process, and take an
  optional `space` for stop/status.
- `send_file` lands through TeleportService in `~/Downloads[/subdir]`;
  `target_directory` must be under Downloads (use `upload` otherwise). No
  25 MiB cap anywhere.
- `upload` / `download` answer JSON reports; `call_tool` propagates the
  tool's `isError`.
- `teleport_app` needs `acknowledge_sensitive: true` when the selection
  holds a sensitive item; without `include` the default-checked set moves.
- `list_tools` / `call_tool` reach the spacesd's cua-driver registry
  (`driver`; `mcp`, `cua-driver`, `computer-server` are aliases). The Local
  macOS app MCPs (Blender, Unity) behind ssh stdio are not carried over.
- Agents run as detached, tagged spacesd processes (no tmux, no
  LaunchAgent); the Jev status classifier is not ported (no wired harness
  needs it).
