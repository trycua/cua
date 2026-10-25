# cua-spacesd-server

The gRPC, gRPC-Web and HTTP server core of `cua-spacesd`. One TCP port
(default 3211) serves:

| Surface | Auth | Notes |
|---|---|---|
| `cua.env.v1.*` over native gRPC (h2c) and gRPC-Web (HTTP/1.1) | root token | `authorization: Bearer <token>` **or** `x-cua-env-authorization: Bearer <token>` (a gateway in front of the daemon can strip `authorization`) |
| gRPC reflection (`grpc.reflection.v1`, `v1alpha`) | root token | from `cua_proto::FILE_DESCRIPTOR_SET` |
| `GET /health` | none | 204 while serving, 503 while shutting down |
| `GET/HEAD/PUT/POST /files?path&method&exp&sig` | HMAC signed URL | from `FilesystemService.CreateSignedUrl`; mandatory expiry; `Range`; 507 on a full disk; a URL signed for `PUT` also takes `POST multipart/form-data` (the first file part is written like the `PUT` body, answer `[{"name","path","type"}]`); a URL signed with `run_as` acts as that guest user |
| `POST /mcp` | root token (either header) | streamable-HTTP MCP over the cua-driver registry (`cua_driver_core::server` dispatch) |
| `GET /tunnel` (WebSocket) | `tunnel` ticket | from `TunnelService.Forward` |
| `GET /hotspot` (WebSocket) | `hotspot` ticket | from `TunnelService.StartHotspot` |
| `/media` | provider | 501 until a desktop provider serves it |

Services implemented here: System, Process, Filesystem, Driver, Teleport,
Tunnel. Computer, Windows, Accessibility, Stream and Presence come from an
extension provider; until one is registered they answer
`FAILED_PRECONDITION` with `ErrorInfo{reason: FEATURE_UNSUPPORTED}` and
`GetCapabilities` reports their features unsupported with a limitation.

## Acting as another guest user

Every `FilesystemService` request that touches a path (and `CreateSignedUrl`)
takes an optional `run_as` guest user. The agent starts one short-lived
helper per user (`cua-spacesd fs-as --user NAME`, the same binary, directly
when the agent is root and through `sudo -n` otherwise: the Linux images run
the agent as `cua` with passwordless sudo). The helper drops to that user's
uid, gid and supplementary groups and serves `FilesystemService` over the
socket it was started with; the agent forwards the request and relays the
answer, so the kernel's checks are that user's and created files are owned
by them. Uploads and watchers stay in their helper and later calls are routed
there. Without the privilege the call fails `PERMISSION_DENIED` ("cannot act
as ...") with the reason; `fs.run_as` in `GetCapabilities` reports the
privilege (`root`, `sudo`, or `none: <why>`). Viewer tickets may not use
`run_as`.

## File metadata

`WriteFileHeader.metadata` is stored as extended attributes under
`xattr_namespace` (default `user.cua.meta.`; must start with `user.` and end
with `.`), published atomically with the file, and returned in
`EntryInfo.metadata` (`Stat`, `ListDir`, `ReadFile`, the write responses) and
`FsEvent.metadata`. Limits: 32 keys, keys of 1 to 128 bytes of
`A-Za-z0-9._:/@+-`, values up to 1024 bytes, 3072 bytes of keys and values in
all. A file system without extended attributes refuses a write that carries
metadata (`FAILED_PRECONDITION`) and reports none.

## Token and bind rules

- The token is read from `CUA_ENV_TOKEN`, `/run/cua/env-token` (or
  `--token-file`), `--token`, or installed later by `SystemService.Init`.
  Comparisons are constant time.
- With a token the default bind is `0.0.0.0:3211`, without one
  `127.0.0.1:3211`. A non-loopback bind without a token is refused, unless
  `--insecure-bootstrap` is set; then only `GetCapabilities`, `Health` and
  `Init` answer until `Init` installs a token.
- Tickets and signed URLs are keyed by an HMAC key derived from the token,
  so rotating the token (via `Init`) revokes them all.

## Await-token-file mode

`--await-token-file` (or `CUA_ENV_AWAIT_TOKEN_FILE=1`) takes the token only
from `--token-file`, which the orchestrator writes (for example a Secret
mounted at `/run/cua/env-token`, empty until a claim binds). Tokens are never
accepted over the network in this mode.

- Binds `0.0.0.0:3211` with no token. While the file is empty or missing,
  only `GetCapabilities` (`initialized=false`) and `Health` (component
  `auth`: `awaiting token file ...`, overall still `SERVING`) answer.
  Everything else is `FAILED_PRECONDITION` / `NOT_INITIALIZED`.
- The file is polled every 500 ms (`CUA_ENV_TOKEN_POLL_MS`). inotify only
  wakes the poll early, since gVisor may not deliver events. A changed file
  is re-read until stable before it is applied.
- Valid token: trimmed, 16 to 4096 bytes of `[A-Za-z0-9-._~+/=]`, regular
  file, not accessible to every user (allowed only as root in a container).
  Anything else counts as no token (fail closed) and is logged.
- Install: the token goes live. Rotate: the old token, its tickets and
  signed URLs stop working, and every session opened with it is revoked:
  media sockets close with 4401, forwards and the hotspot stop, and open
  HTTP/gRPC connections are dropped. Empty or missing: the same revocation,
  `Init` defaults reset, and the driver returns to awaiting.
- `Init` may still set defaults (env, user, labels), but a token different
  from the current one is `PERMISSION_DENIED`.
- `cua-spacesd token-sync --from <root-only file> --to <file> --owner cua`
  is the privileged helper: it runs as root, mirrors the source to a `0600`
  file owned by `--owner` in a root-owned directory, and empties the target
  when the source is empty, missing or invalid. The unprivileged driver
  awaits the target.

## Extension point for the desktop services

```rust
use cua_spacesd_server::{ServerBuilder, ServerConfig, ServerContext, ServiceProvider};
use cua_spacesd_server::{axum, tonic};

pub trait ServiceProvider: Send + Sync + 'static {
    // Required
    fn capabilities(&self) -> Vec<Feature>;
    fn register(&self, routes: tonic::service::Routes, ctx: &ServerContext) -> tonic::service::Routes;
    fn http_routes(&self) -> Option<axum::Router>;
    // Provided (override as needed)
    fn services(&self) -> Vec<&'static str> { vec![] }     // names added in `register`
    fn http_paths(&self) -> Vec<&'static str> { vec![] }   // paths served by `http_routes`
    fn displays(&self) -> Vec<Display> { vec![] }          // GetCapabilities.displays
    fn display_server(&self) -> Option<DisplayServer> { None }
    fn health(&self) -> Vec<ComponentHealth> { vec![] }    // Health components
    fn tool_provider(&self) -> Option<Arc<dyn ToolProvider>> { None } // share your registry
}
```

Wiring (in `crates/cua-spacesd/src/main.rs`):

```rust
let ctx = ServerContext::new(config, token);
let desktop = Arc::new(DesktopServices::new(ctx.clone(), bundle)); // your provider
let server = ServerBuilder::new(ctx.clone())
    .provider(desktop)          // registers Computer/Windows/A11y/Stream/Presence
    .tools(registry)            // or let the provider's tool_provider() supply it
    .build();
server.serve(cua_spacesd_server::bind(&ctx).await?).await?;
```

Rules the server enforces or relies on:

1. **gRPC services.** Add them in `register` with the generated
   `*ServiceServer::new(..)` (set `max_decoding_message_size` to
   `config::MAX_MESSAGE_BYTES`) and list every fully-qualified name
   (`"cua.env.v1.StreamService"`) in `services()`. A desktop service that no
   provider lists gets the FAILED_PRECONDITION stub; listing one you did not
   register leaves it UNIMPLEMENTED, which the route/spec test catches;
   registering a service twice panics at build time.
2. **Auth is done for you on gRPC.** Every gRPC call has passed the token
   check before your handler runs. Read the caller with
   `cua_spacesd_server::caller(&request)`, which gives `principal:
   Option<Principal>` from `x-cua-principal-bin`.
3. **HTTP routes are not authenticated by the server.** `/media` and any
   other route must validate a ticket itself:
   ```rust
   let (claims, subprotocol) = ctx.validate_request_ticket(&uri, &headers, TicketScope::Media)?;
   // claims.resource is the media session id you minted; echo `subprotocol`
   // (the `cua.ticket.<ticket>` entry) when accepting the WebSocket.
   ```
   Mint tickets from `StreamService.OpenMedia` with
   `ctx.mint_ticket(TicketScope::Media, &session_id, principal.as_ref(), ttl)`.
   Tickets are accepted in the `ticket` query parameter or as the WebSocket
   subprotocol `cua.ticket.<ticket>`. Use `ctx.check_bearer(&headers)` for
   routes that take the root token instead. Every path must be one of
   `cua_proto::metadata::*_PATH` and listed in `http_paths()`.
4. **Audio uplink.** `SystemService.Init(audio_uplink)` is stored in the
   context; call `ctx.audio_uplink_allowed(principal.as_ref())` in
   `OpenMedia` before accepting `audio.uplink.enabled`.
5. **Counters and lifecycle.** Increment/decrement
   `ctx.media_sessions()` for `Metrics.media_session_count`; stop background
   work when `ctx.shutdown_token()` is cancelled.
6. **Capabilities.** Report every desktop feature you know about
   (`a11y`, `background_input`, `desktop_stream`, `window_stream`,
   `h264_hw`, `h264_sw`, `quic_media`, `clipboard.text`, `clipboard.files`,
   `presence`, `windows`, `launch_app`, `audio.desktop`, `audio.per_app`,
   `audio.uplink`, `audio.opus`), with a `limitation` when unsupported. Your
   entries replace the server's "no desktop provider" defaults by name.

## `/share/*` (cua share)

An extension route, not part of the env spec's path list. While the host
policy (`host.json`) has `"shares": true` and `"shares_gateway":
"<socket>"` (the cua daemon's share gateway, a Unix socket), `ANY /share/*`
is reverse-proxied to that socket; otherwise it answers 404 (403 once the
machine stopped sharing, 502 while the daemon is down). It works on a
share-only host (`share_desktop: false`, `provide_spaces: false`), which
exposes nothing else.

- `/share/<id>/…`: needs a verified caller, either the relay's signed
  assertion or the machine's env token. The gateway receives it as JSON in
  `x-cua-share-caller` (`account`, `email`, `name`, `role` = `owner` /
  `shared` / `viewer`, `teams`, `via` = `relay` / `token`). The header is
  always replaced: a client-supplied value never reaches the gateway.
- `/share/pub/<token>/…`: no credential; the token is the credential and the
  gateway checks it. No caller header is sent.
- Client credentials (`authorization`, the assertion, the env token header,
  device session) are removed before forwarding. Request and response bodies
  stream; WebSocket upgrades are spliced through.
- Each forwarded request is one access-log line with a public token cut to
  its first six characters.

## Tests

```sh
cargo test -p cua-spacesd-server                          # unit + conformance + relay (host-safe)
CUA_ENV_TEST_TARGET=http://host:3211 CUA_ENV_TEST_TOKEN=… \
  cargo test -p cua-spacesd-server --test conformance     # against any running driver
scripts/ci/linux-core-tests.sh                        # Linux, 1 GiB transfers, 60 s stream
scripts/ci/relay-e2e.sh                               # machine without published ports behind cua-relay
```

The conformance suite starts an in-process server with temp directories, a
fake teleport host and a fake tool registry; it runs only `sh`, `stty`,
`python3 -m http.server` and `curl` against loopback.
