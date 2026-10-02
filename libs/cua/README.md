# libs/cua: the cua SDK workspace

This Cargo workspace holds the cua SDK, the `cua` CLI and `cua daemon`, and
the protobuf contract for **cua-spacesd**, the daemon that runs inside
sandboxes (`libs/cua-spacesd`). Architecture overview:
[docs.cua.ai/concepts/architecture](https://cua.ai/docs/cua-sdk/concepts/architecture).

```text
 Python / Node / Swift / Kotlin / wasm
        |  UniFFI (cua-sdk is the only #[uniffi::export] crate)
 +------+------------------------------------------------------------+
 | cua-sdk -- cua-sandbox-core -- cua-vmm / cua-image / cua-fleet     |
 |    |              +-- cua-spacesd-client (client) -- cua-proto (generated)    |
 |    +-- cua-spaces, cua-teleport, cua-daemon (same runtime on a UDS) |
 +------------------------------+-------------------------------------+
          gRPC / gRPC-Web on :3211, media on /media or QUIC :3212
 +------------------------------+-------------------------------------+
 | cua-spacesd (libs/cua-spacesd): server only, depends on       |
 | cua-proto and the cua-media-* crates, input through cua-driver      |
 +---------------------------------------------------------------------+
```

- **Embedded or daemon.** `Cua.embedded()` runs everything in your process;
  `Cua.connect()` talks to `cua daemon` (UDS `~/.cua/cua.sock`, 0600, or
  loopback + token), which shares sandboxes, env connections, tunnels, streams
  and credentials across processes. Same API either way.
- **Sandboxes are daemon-agnostic.** Lifecycle and readiness never assume
  cua-spacesd. `sb.spacesd()` probes `SystemService.GetCapabilities` and fails
  with `SpacesdNotAvailable` if the driver is missing. A Space works on any
  image too; only the Spaces primitives (shell, files, streams, presence,
  teleport, hotspot, agents) need it.
- **Zero pre-setup local runtimes.** `cua-vmm` starts `lume serve` on macOS,
  fetches QEMU when absent, and runs container images with Docker/Podman and
  gVisor `runsc` when available (`cua runtime doctor|setup` shows each step).

## Sandboxes (Rust)

The same options local and in the cloud; only `on` changes (`kind` and `runtime` pick the machine, `auto` by default):

```rust
use cua_sdk::{CloudOptions, Cua, CuaConfig, ReadinessProbe, SandboxCreateOptions};
use std::collections::HashMap;

let cua = Cua::embedded(CuaConfig::default())?;
let sb = cua.sandboxes().create(SandboxCreateOptions {
    command: Some(vec!["python".into(), "-m".into(), "my_mcp".into()]),
    env: HashMap::from([("FOO".into(), "bar".into())]),
    services: HashMap::from([("mcp".into(), 8765)]),
    wait_for: vec![ReadinessProbe::http("mcp", "/health")],
    cloud: Some(CloudOptions { warm: Some(true), ..Default::default() }), // ignored locally
    ..SandboxCreateOptions::new("local", "python:3.12-slim")   // or "cloud"
}).await?;
let svc = sb.service("mcp".into())?;
let r = svc.request("POST".into(), "/mcp".into(), Some(body), None, Some(headers)).await?;
let url = svc.url().await?;                                  // usable from this machine
let share = sb.public_url("mcp".into(), Some(3600), None).await?; // shareable, expires
let fwd = sb.forward(8765).await?;                           // loopback URL: fwd.url()
let info = sb.info(); // id, phase, location, services, expires_at_unix, provider_details
```

- `public_url` is a signed URL in the cloud and, locally, a loopback URL
  with its own token served by the `cua daemon` (started on demand).
- `sb.mcp(service, None)` is an MCP client (the official Rust SDK, rmcp) and
  `sb.mcp_config(service, None)` the URL and headers for any MCP client; no
  cua-spacesd needed.
- `sidecars: vec![Container { .. }]` adds containers addressed by name on
  every runtime: the sandbox reaches a sidecar at its `name`, a sidecar
  reaches the sandbox at `main`, and with sidecars the service names `main`,
  `sidecars` and `sc` are reserved. Local ones share the sandbox's network
  namespace and need `runtime: Some("runc".into())` where gVisor would run; the cloud runs
  them on gVisor and KubeVirt. `registry_secret` pulls a private image.
- Cloud `command`, `env` and args run on both runtimes (`processMode: Run`,
  sent whenever they are set). `build` adds layers locally; cloud remote
  builds wait on the cloud builder (`cua-fleet` feature `fleet-remote-builds`)
  and fail with a clear error until then.
- `Image.linux()` and the CLI alias `linux` resolve to
  `ghcr.io/trycua/linux:24.04` (`windows:2022`, `macos:26`) through one
  resolver, `cua_image::resolve`, which picks the variant a backend runs.

Docs: [Sandboxes](https://cua.ai/docs/cua-sdk).

## Crate map

| Crate | Contents |
|---|---|
| `cua-proto` | `proto/` codegen: prost messages and tonic clients and servers for `cua.env.v1` and `cua.daemon.v1`. Also the descriptor set, well-known constants (ports, metadata keys, HTTP paths) and the client-stream fallback registry. |
| `cua-spacesd-client` | Ergonomic spacesd client: reconnect, keepalive, chunked transfer, gRPC vs gRPC-Web, auth, and the Fleet gateway adapter. |
| `cua-fleet` | Fleet pools, templates, claims and images over `libs/fleet/sdk` (cyclops-sdk, a read-only mirror this crate path-depends on). Auto-managed pools and the one runtime/image pairing check. |
| `cua-vmm` | Local runtimes: Lume, QEMU and OCI containers (gVisor when available). |
| `cua-image` | OCI pull and push, containerDisk and rootfs formats, `ImageSpec`, local builder. |
| `cua-sandbox-core` | One daemon-agnostic `Sandbox` over the `local`, `cloud` and `direct` locations, and the placement model (`on`, `kind`, `runtime`) and user defaults (`cua config`); cua-sandbox-compatible state files (`~/.cua/sandboxes`). |
| `cua-spaces` | The Spaces core: registry (`~/.cua/spaces.json`), create/add/delete/remove, capability-gated primitives over cua-spacesd-client, and the Spaces MCP server. See its [README](crates/cua-spaces/README.md). |
| `cua-hotspot` | The hotspot (reverse-SOCKS) tunnel frame codec and egress peer, shared with cua-spacesd. |
| `cua-spaces-contract` | The Spaces tool manifest (33 tools, schemars input schemas) generated to [`spaces-contract/manifest.json`](spaces-contract/manifest.json) with a `--check` gate. |
| `cua-spaces-transcript` | VT emulator, cast player and Claude Code frame parser with cross-language fixtures in `spaces-contract/fixtures/transcript`. |
| `cua-teleport` | Teleport SEND: export providers, keychain read, approval gate, upload over `TeleportService` (`cua teleport push`). See its [README](crates/cua-teleport/README.md). Receive lives in cua-spacesd. |
| `cua-teleport-bundle` | Effect-free bundle format and per-app layout data shared by sender and receiver. |
| `cua-host` | Unattended access (`cua host`): installs cua-spacesd as a per-OS service that joins a cua-relay as your account or serves on a direct ip:port, plus the relay machine-directory client. |
| `cua-auth` | Sign in to Cua (browser PKCE with a loopback redirect, device-code fallback), refresh, and the credential store shared by the CLI, SDK, daemon and Spaces app. |
| `cua-agent-setup` | Agent onboarding: detect AI coding agents, install the bundled skills (`skills/`, synced by `scripts/sync-skills.sh`) and configure the cua MCP server (`cua agents`). See its [README](crates/cua-agent-setup/README.md). |
| `cua-media-protocol`, `cua-media-transport`, `cua-media-codec` | Media plane shared with cua-spacesd: wire v2 types, framing (RVD2 video, RAU2 audio, QUIC), encoder probing/selection and Opus. |
| `cua-media-client`, `cua-viewer`, `cua-logging` | Media client state and decode, the native proxy-window viewer, shared file logging. |
| `cua-daemon` | The SDK runtime and its host: `cua.daemon.v1` on the socket and loopback, env passthrough (`/v1/sandboxes/<name>/env/...`, `/v1/spaces/<key>/env/...`), the ticketed webview media bridge, the Spaces MCP server at `/mcp` and `cua daemon mcp`. Feature `test-fixtures` adds loopback fixtures; the `cua-test-fixtures` binary lives in `libs/cua-spacesd/tests/spaces-e2e` (build it with `scripts/build-test-fixtures.sh`). |
| `cua-sdk` | The **only** `#[uniffi::export]` crate (UniFFI `=0.31.0`): `Cua.embedded` / `Cua.connect`, sandboxes, env (typed plus a proto3-JSON escape hatch over every `cua.env.v1` RPC), media sessions with `FrameSink`/`AudioSink`, Fleet, local runtimes and images, Spaces, auth and agent setup; a wasm32 browser subset. |
| `cua-bindgen` | The pinned UniFFI CLI used by `scripts/generate-uniffi-bindings.mjs`. |
| `cua-cli` | The `cua` binary (shipped in the `cua` wheel, `@trycua/cua`, the install scripts and the Spaces app): `auth`, `agents`, `sandbox`, `config`, `do`, `mcp`, `skills`, `trajectory`, `wif-token`, `fleet`, `image`, `env`, `daemon`, `runtime`, `host`, `spaces`, `teleport`. Reference: [docs.cua.ai/reference/cua-cli](https://cua.ai/docs/cua-cli/reference/cli). |

Language packages generated from `cua-sdk` (drift-checked by
`scripts/generate-uniffi-bindings.mjs --check`):

| Package | Path | Notes |
|---|---|---|
| PyPI `cua` | [`python/`](python/README.md) | `pip install "cua[sandbox]"` adds `from cua import Sandbox, Image` |
| npm `@trycua/cua` | [`typescript/`](typescript/README.md) | `@trycua/cua/browser` (wasm, via `scripts/build-wasm.mjs`), `@trycua/cua/spaces` |
| Swift `Cua` | [`swift/`](swift/README.md) | XCFramework via `swift/scripts/build-xcframework.sh` |
| Kotlin | [`kotlin/`](kotlin/README.md) | Generated only, no published artifact yet |

CI: `.github/workflows/ci-cua-sdk.yml`. Release: component `cua-sdk`,
`.github/workflows/cd-cua-sdk.yml`. The toolchain is pinned in
`rust-toolchain.toml`, in lockstep with `libs/cua-driver/rust`. Streaming
benchmarks: [`bench/streaming`](bench/streaming/README.md).

## The contract (`proto/`)

The `.proto` files are the source of truth. Code is always generated from
them and never written by hand.

| Package | Files | Services |
|---|---|---|
| `cua.env.v1` | `common`, `system`, `process`, `filesystem`, `computer`, `windows`, `accessibility`, `driver`, `stream`, `presence`, `teleport`, `tunnel`, `volume` | `SystemService`, `ProcessService`, `FilesystemService`, `ComputerService`, `WindowsService`, `AccessibilityService`, `DriverService`, `StreamService`, `PresenceService`, `TeleportService`, `TunnelService`, `VolumeService` |
| `cua.daemon.v1` | `sandboxes`, `spaces`, `runtime`, `daemon` | `SandboxService`, `SpaceService`, `RuntimeService`, `DaemonService` |

Rules. `common.proto` has the full text of each.

- **Transport.**
  - Native gRPC and gRPC-Web are served on one port.
  - gRPC-Web cannot carry client streams. Every client-streaming RPC therefore
    has a unary, chunked fallback, registered in
    `cua_proto::CLIENT_STREAM_FALLBACKS`. A test fails if one is missing.
  - Bidirectional RPCs are forbidden.
- **Auth.**
  - RPCs use `authorization: Bearer <token>` or `x-cua-env-authorization`
    (the Fleet gateway strips `authorization`).
  - The media, tunnel and hotspot sockets use short-lived tickets from
    `OpenMedia`, `Forward` and `StartHotspot`.
  - `x-cua-principal-bin` carries a serialized `Principal`.
- **Errors.** Errors are gRPC status codes plus `google.rpc.Status` details
  that pack a `cua.env.v1.ErrorInfo`.
- **Evolution.**
  - Changes are additive only. `buf breaking` (FILE + WIRE_JSON) runs against
    the merge base.
  - Removed fields are `reserved`.
  - Numbers 1000-1999 are for experimental fields.
  - The zero value of every enum is `*_UNSPECIFIED`.
  - Every element carries a doc comment. This is enforced by the `COMMENTS`
    lint category.
- **Media.** Video and audio stay off gRPC. See [`proto/MEDIA.md`](proto/MEDIA.md)
  for media wire v2 (`rcdp.v2` on the wire): tickets, desktop targets, keyframe on attach,
  sequences that may start anywhere, and audio tracks (Opus/PCM downlink,
  optional microphone uplink) on a shared microsecond media clock.

## Regenerating and checking

Code generation runs in `crates/cua-proto/build.rs` on every build. It needs
`protoc` on `PATH`, or `PROTOC` set.

```bash
cd libs/cua
cargo build                              # regenerates into OUT_DIR
cargo test -p cua-proto --all-features   # round-trips + descriptor checks
scripts/check-proto.sh                   # buf lint, format --diff, breaking
scripts/check-proto.sh --fix             # apply buf format
```

When you add a `.proto` file:

1. List it in `PROTOS` in `build.rs`. A test compares that list with the
   files on disk.
2. If it adds a service, add the service to the lists in
   `tests/descriptor.rs`.

The `serde` feature adds canonical proto3-JSON impls through pbjson.
Well-known types map to `pbjson_types`, which is wire-identical to
`prost_types`, so the generated types are the same with or without the
feature.

The generated code references no `tonic::transport` items, so it builds for
`wasm32-unknown-unknown`. CI checks this.

CI: `.github/workflows/ci-cua-proto.yml`.
