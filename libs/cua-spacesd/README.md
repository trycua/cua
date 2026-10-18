# cua-spacesd

The daemon that runs inside a cua sandbox (or on any machine you want to reach
as a Space). One binary serves the `cua.env.v1` contract
([`libs/cua/proto`](../cua/proto)) on one port, **3211**:

| Surface | What |
|---|---|
| gRPC and gRPC-Web (same port) | `System`, `Process`, `Filesystem`, `Computer`, `Windows`, `Accessibility`, `Driver`, `Stream`, `Presence`, `Teleport`, `Tunnel` services, plus gRPC reflection |
| `GET /health` | Unauthenticated, 204 while serving |
| `/files` | Signed-URL upload and download (HMAC, mandatory expiry) |
| `/mcp` | Streamable-HTTP MCP over the cua-driver tool registry |
| `/tunnel`, `/hotspot` | Port forwarding and reverse SOCKS egress (ticketed WebSockets) |
| `/media`, QUIC on 3212 | Video (H.264) and audio (Opus) on media wire v2 ([`MEDIA.md`](../cua/proto/MEDIA.md)) |

It replaces the former in-image services (computer-server on 8000, the rcdp
daemons, and the supergateway-wrapped cua-driver MCP) and is not
wire-compatible with them. Clients use the cua SDK's `spacesd` module
([`libs/cua`](../cua/README.md)) or `cua spacesd ...`; this workspace is
server-only. Sandboxes do not require it: the cua SDK treats it as an optional
attachment that only Spaces features (streams, presence, teleport, file send,
hotspot, agents) need.

The driver never injects input itself. Pointer, keyboard and other actions are
delegated to [cua-driver](../cua-driver/README.md) (`cua-driver-core` and
`platform-{linux,macos,windows}`), linked in-process.

## Run

```sh
CUA_ENV_TOKEN=... cua-spacesd         # 0.0.0.0:3211 (127.0.0.1:3211 without a token)
cua-spacesd --print-config            # effective configuration, never the token
cua-spacesd --help
```

| Setting | Flag / env | Default |
|---|---|---|
| Listen address | `--listen` / `CUA_ENV_LISTEN` | `0.0.0.0:3211` with a token, `127.0.0.1:3211` without |
| Port | `--port` / `CUA_ENV_PORT` | `3211` |
| QUIC media port | `--quic-port` / `CUA_ENV_QUIC_PORT` | `3212` (`0` disables QUIC) |
| Token | `CUA_ENV_TOKEN`, `--token-file` / `CUA_ENV_TOKEN_FILE`, `--token` | `/run/cua/env-token` |
| Data directory | `--data-dir` / `CUA_ENV_DATA_DIR` | per-user state directory |
| Teleport destination | `--downloads-dir` / `CUA_ENV_DOWNLOADS_DIR` | `~/Downloads` |
| Disable parts | `--no-mcp`, `--no-driver`, `--no-desktop` | all on |
| Allow guest power-off/reboot | `--allow-guest-power` | off |

### Token

- A token is **required for any non-loopback bind**. Clients send it as
  `authorization: Bearer <token>` or `x-cua-env-authorization: Bearer <token>`
  (the Fleet gateway strips `authorization`). Comparisons are constant time.
- `SystemService.Init` can install a token later. `--insecure-bootstrap` allows
  a tokenless non-loopback bind that answers only `GetCapabilities`, `Health`
  and `Init` until then (opt-in, for an authenticated gateway only).
- `--await-token-file` (Fleet): take the token only from `--token-file`, bind
  while it is empty, install or rotate it when the file changes, and revoke
  every session when it is emptied. `cua-spacesd token-sync` is the
  privileged helper that mirrors a root-only secret to a file the driver's user
  can read.
- Media, tunnel and hotspot sockets and `/files` use short-lived tickets or
  signed URLs keyed from the token, so rotating the token revokes them.

Details: [`crates/cua-spacesd-server/README.md`](crates/cua-spacesd-server/README.md).

### Reach it through a relay

`join` serves locally and also publishes the machine through a
[`cua-relay`](crates/cua-relay) over one outbound WSS connection (yamux), so a
machine behind NAT needs no inbound port:

```sh
CUA_ENV_TOKEN=... cua-spacesd join --relay wss://relay.example --relay-token ...
```

Clients reach it at `https://<relay>/m/<machine-id>/` with the env token as
usual. In account mode (`cua host setup`), the relay authenticates machines
and clients with cua.ai accounts and forwards a relay-signed principal
assertion that the driver checks against the host policy (owner, allowlist,
stop sharing), so env tokens never leave the host. The relay binary, its
Dockerfile and a Kubernetes manifest (`deploy/relay.k8s.yaml`) live in
`crates/cua-relay`.

## Capabilities

`System.GetCapabilities` reports the version, OS, runtime (kubevirt, gvisor,
lume, qemu, bare), displays and each feature (`a11y`, `background_input`,
`desktop_stream`, `window_stream`, `h264_hw`, `h264_sw`, `quic_media`,
`clipboard.*`, `presence`, `presence.cursor_shape`, `audio.desktop`, `audio.per_app`, `audio.uplink`,
`teleport.<provider>`, ...). An unsupported feature carries a `limitation`
string instead of failing silently. Check capabilities rather than the
platform.

## Platforms

| Platform | Capture and encode | Service |
|---|---|---|
| Linux | X11 (XShm/Composite) desktop and window targets; Hyprland displays and windows via `grim`, focus and geometry via `hyprctl`, text clipboard via wl-clipboard; OpenH264; PipeWire/Pulse audio | systemd ([`packaging/linux`](packaging/linux)) |
| macOS | ScreenCaptureKit, VideoToolbox H.264, ScreenCaptureKit audio | LaunchAgent in the GUI session ([`packaging/macos`](packaging/macos)); needs Screen Recording and Accessibility grants for a stable signed identity |
| Windows | Windows.Graphics.Capture, OpenH264; WASAPI loopback audio | Scheduled task in the interactive session ([`packaging/windows`](packaging/windows)) |

Encoders are probed fastest first (NVENC, VA-API, QSV, AMF, VideoToolbox,
Media Foundation, OpenH264). Hardware sessions other than VideoToolbox are
probe-only for now, so the defaults are VideoToolbox on macOS and OpenH264
elsewhere. Background input, accessibility and window management follow
cua-driver's per-platform support.

The reference Linux image (XFCE on Xvfb, PipeWire and the driver; the HTML5
viewer is served by cua-spacesd at `/viewer/`) is
[`libs/images/linux`](../images/linux).

## Presence cursor shapes

Participants that join presence with `cursor_shapes` get the shape the guest
would show at each cursor (arrow, I-beam, hand, resize, busy, progress,
not-allowed, crosshair, grab, move), their own included. For each cursor,
cheapest first:

1. **System**: the participant's own input put the real pointer there, so the
   OS's real cursor is read.
2. **Probe**: the pointer is idle (nothing injected for 750 ms, nothing
   pending), so it is moved to the cursor for about 40 ms, the real cursor is
   read, and it is moved back to the exact previous position. Input and
   probes share one lock; input that arrives mid-probe aborts it and the
   pointer is restored first. At most one probe runs at a time, and a
   participant is probed at most every 100 ms.
3. **Hit-test**: the accessibility element under the cursor (text -> I-beam,
   link or button -> hand) and window edges (-> resize). Never moves the
   pointer.

Results are cached per element or 16 px cell for 1 s. The capability
`presence.cursor_shape` names the backends in its `hit_test`, `system` and
`probe` attributes (`probe` is `off` when disabled). Every published image
claims the capability and pins its backends in `image.json`
`claims.feature_attributes`, and `cua-spacesd doctor --strict` fails the
release when the daemon reports other ones (`capabilities.attributes.presence.cursor_shape`).

| Desktop | Hit-test | Real cursor | Probe | Limitation |
|---|---|---|---|---|
| Linux X11 | AT-SPI | XFixes cursor name | XTest | Toolkits without AT-SPI (plain X clients) hit-test as the arrow; the probe still reads their real cursor |
| Linux Hyprland | AT-SPI with `hyprctl` window geometry | none | none | Wayland does not expose the compositor's cursor shape to clients |
| Other Wayland (GNOME, KDE) | none | none | none | No window geometry or cursor shape for clients: every cursor is the arrow |
| macOS | Accessibility (`AXUIElementCopyElementAtPosition`) | `NSCursor.currentSystemCursor` | `CGWarpMouseCursorPosition` | Needs the Accessibility grant; the real cursor updates only after the app under the pointer sets it (the probe's dwell covers it) |
| Windows | UI Automation plus `WM_NCHITTEST` | `GetCursorInfo` | `SetCursorPos` | App-defined cursors are not classified; elevated apps and the secure desktop are opaque to a non-elevated daemon |

The probe is a per-Space setting, on by default: `InitRequest.presence.cursor_probe`
changes it at run time, `--cursor-probe false` (or
`CUA_SPACESD_CURSOR_PROBE=0`) sets the initial value. It gives exact,
app-specific shapes that the hit-test can only guess, at the cost of a brief
hover under an idle pointer (hover highlights may flash). Turn it off for
Spaces where any pointer movement nobody asked for is unwanted; shapes then
come from the hit-test alone.

Staleness: a participant leaves with a reason (`LEFT`, `DISCONNECTED`,
`TIMEOUT`, `RUN_ENDED`). Agent cursors leave when their driver session or
agent run ends (`DELETE /mcp` with `X-Cua-Agent-Session` and no
`Mcp-Session-Id` ends a run) or after 15 s idle; a human cursor idle for 60 s
is hidden. `roster_interval` heartbeats let clients drop anyone they missed
leaving. Native clients can move cursors to the QUIC datagram channel
(`cursor_datagrams`, ALPN `cua-presence/1` on the media port). Wire details:
[`libs/cua/proto/PRESENCE.md`](../cua/proto/PRESENCE.md).

## Install and package

```sh
packaging/install.sh --binary ./cua-spacesd --token-file token   # or --version X.Y.Z
packaging/install.sh --dry-run ...                                   # show the plan
```

`install.ps1` is the Windows equivalent. For unattended access to your own
machine use `cua host setup` instead: it installs the same service and joins
the cua.ai relay as your account. `scripts/build-macos-app.sh` builds the
signed `Cua Spacesd.app`; `scripts/build-windows.ps1` stages a Windows
package.

Releases are built by `.github/workflows/cd-cua-spacesd.yml` for tag
`cua-spacesd-v<version>` (release-please component `cua-spacesd`).
`VERSION` and the workspace version in `Cargo.toml` always match; release-please
bumps both. `packaging/release/package.sh` lists the assets and which installer
uses each. `packaging/release/build-linux-docker.sh` reproduces the Linux job
locally in debian:11.

The `cua` CLI bakes `VERSION` and `cua host setup` downloads that release, so
publish cua-spacesd before releasing Cua Spaces or the cua SDK:
`cd-cua-spaces.yml`, `cd-cua-sdk.yml` and the Release Please PR check
(`ci-release-spacesd-pin.yml`) fail while the pinned release is missing, a
draft, or lacks a host-setup asset
(`.github/scripts/check_spacesd_published.sh`). An already-shipped CLI whose
pin is unavailable falls back to the newest published release on the same
major.minor, else the closest newer one on the same major.

## Crates

This workspace is server-only. Wire formats, codecs and clients live in
`libs/cua` (`cua-media-protocol`, `cua-media-transport`, `cua-media-codec`,
`cua-media-client`, `cua-teleport-bundle`) and this workspace depends on them
by path, never the reverse.

| Crate | Role |
|---|---|
| `cua-spacesd` | The binary: `serve` (default), `join`, `token-sync`, `legacy` |
| `cua-spacesd-server` | gRPC/gRPC-Web/HTTP server core, auth, tickets, System/Process/Filesystem/Driver/Teleport/Tunnel services, conformance tests |
| `cua-spacesd-desktop` | Computer, Windows, Accessibility, Stream and Presence services; capture, encode and the cua-driver adapters |
| `cua-spacesd-session` | Session state, freshness checks, action correlation, bounded delivery |
| `cua-spacesd-provider-api` | Target, capture, action and accessibility provider traits |
| `cua-spacesd-teleport` | Teleport receive: verify and import app-session bundles, relaunch, file-transfer path rules. Send lives in the cua SDK (`cua-teleport`, `cua teleport push`) |
| `cua-spacesd-socks` | Hotspot (reverse SOCKS) listener |
| `cua-relay` | The reverse-tunnel relay and machine directory |
| `cua-spacesd-test-apps` | Fixture apps for the desktop tests |

## Build and test

Needs `protoc` (the contract is generated at build time). The macOS build
needs the macOS 26 SDK (ScreenCaptureKit bindings).

```sh
cd libs/cua-spacesd
cargo build -p cua-spacesd
cargo test --workspace --locked
scripts/ci/linux-core-tests.sh    # Linux container: large transfers, long streams
scripts/ci/relay-e2e.sh           # a driver with no published ports behind cua-relay
cargo test -p cua-spacesd --test windows_e2e -- --ignored --test-threads=1   # interactive Windows desktop only
```

Run the conformance suite against any driver with
`CUA_ENV_TEST_TARGET=http://host:3211 CUA_ENV_TEST_TOKEN=... cargo test -p cua-spacesd-server --test conformance`.
`tests/teleport-e2e` is a separate workspace that runs SDK send against
driver import. CI: `.github/workflows/ci-cua-spacesd.yml` (Linux, macOS,
Windows, relay E2E). Debug a running driver with the `cua` CLI:

```sh
cua spacesd caps http://127.0.0.1:3211 --token TOKEN
cua spacesd targets http://127.0.0.1:3211 --token TOKEN --windows
cua spacesd call http://127.0.0.1:3211 WindowsService/ListWindows --token TOKEN
```

## Legacy window-streaming modes

`cua-spacesd legacy ...` keeps the pre-consolidation modes (owner-only Unix
socket, app-scoped WebSocket and QUIC listeners, Tailscale-authenticated
remote app shares) until the desktop services cover them.
[Remote app shares](docs/remote-app-shares.md) and
[Native app client](docs/native-app-client.md) describe them.

## Design notes

[`docs/`](docs) holds the imported design history: [architecture](docs/architecture.md),
[protocol v1](docs/protocol-v1.md) (media wire v2 changes are in
[`MEDIA.md`](../cua/proto/MEDIA.md)), the [low-latency plan](docs/low-latency-plan.md),
[streaming design notes](docs/streaming-design-notes.md) and the
[cua-driver integration plan](docs/cua-driver-integration-plan.md). They record
decisions at the time they were written; this README and the crate READMEs describe
current behavior.

## License

Source-available under FSL-1.1-MIT ([LICENSE](LICENSE), see
[LICENSING.md](../../LICENSING.md)). Code extracted from the MIT-licensed Cua
prototype keeps its notice; see [Third-party notices](THIRD_PARTY_NOTICES.md).
