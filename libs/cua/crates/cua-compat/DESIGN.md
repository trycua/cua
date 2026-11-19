# cua-compat: run other providers' SDKs, unchanged, on Cua Spaces

`cua-compat` speaks a sandbox provider's wire protocol and serves every request from Cua Spaces through the public Cua SDK. Code written for that provider's SDK then runs on your own machines (and your pool, and your BYOC clouds) with no code change. It is the opposite of `cua-contrib`, which runs Cua sandboxes *on* those providers.

## The one command

```
cua run --compat e2b -- python -m my_rollouts.train config.json
cua run --compat daytona --on host:mac-studio -- uv run agent.py
```

- `cua run -- CMD` runs the command as a job in a Space on your pool, with the current directory (git's files) and its uv project installed. `--env KEY` passes a secret from your shell without printing it. `--on`, `--kind`, `--cpus` and `--memory` work as for every run.
- `--compat e2b|daytona|modal` gives the job the provider's API. Its SDK finds the gateway in the environment and needs no code change. The sandboxes it makes are Cua Spaces wherever the gateway places them: the pool by default, or per sandbox through E2B metadata or Daytona labels (`cua.on`, `cua.kind`, `cua.cpus`, `cua.memory_mb`). That covers any machine, any kind, macOS and Windows images, and BYOC clouds, including E2B, Daytona or Modal through cua-contrib.
- `--compat modal` serves the Modal client's gRPC API (see the Modal notes below and `maps/modal-coverage.md`).

## How it works

```
job Space                       job host (daemon)                    Spaces (any machine)
SDK ─▶ 127.0.0.1:API ─┐                                              
 _cua_ports forwarder ├─ hotspot (dialer: only the gateway's ports) ─▶ cua-compat gateway ─▶ cua SDK ─▶ Space
https://8080-<id>…  ──┘                                                 host router (TLS) ─▶ spacesd tunnel ─▶ guest port
```

1. **Job services.** A job names services (`JobSpec.services`, `HostJobSpec.services = 16`, for example `compat:e2b`).
   - The member runner (`cua_spaces::host_jobs`) asks the provider that the daemon's binary registered (`cua_spaces::job_services`) to start each service. `cua-cli` registers the cua-compat gateway, which is shared and started on first use.
   - The provider returns ports, environment and files. The runner starts the member Space's hotspot with a dialer that reaches only those ports, and runs the command under `_cua_ports.py`.
   - `_cua_ports.py` is a stdlib forwarder that listens on the same ports on the guest's loopback and connects through the hotspot.
   - This works on every kind and OS that runs cua-spacesd, with no new guest code. Core never depends on compat: the dependency points down, through a registration.
2. **The gateway** (`Gateway`, `cua compat serve` for long-running or advanced use).
   - `Backend` is the only code that touches Cua.
   - Spaces are created with `Spaces.create` (pool placement, any kind), or `Sandboxes.create` with `network = "none"`.
   - Fork, checkpoint, suspend, resume and restart go through `Sandboxes`.
   - Processes and files go through the Space's cua-spacesd.
   - Ports go through a cua-spacesd tunnel forward, which reaches a Space on any machine the SDK reaches.
3. **TLS on every name.** The host router serves `<port>-<id>.<domain>` (and `api.`, `49983-<id>.`) over TLS and plain HTTP on one port.
   - It uses a wildcard leaf signed by a local CA that is name-constrained to the gateway domain. Keys are 0600.
   - In a job, `SSL_CERT_FILE` / `REQUESTS_CA_BUNDLE` name a bundle of public roots plus that CA.
   - `*.localhost` resolves to loopback: natively on macOS and with systemd-resolved, and for Python programs started by the forwarder otherwise.
4. **Guest users.** Providers act as named users (E2B `root` and `user`, Daytona `daytona`). When cua-spacesd runs as an ordinary user (the Linux image's `cua`), commands and file operations switch with `sudo -n -u`. Writes are staged and copied into place as that user, so ownership and permission errors match the provider's.
5. **State.**
   - Records (id → Cua name, metadata, access tokens, lifetimes) are kept in `$CUA_HOME/compat/<provider>/state.json` (0600).
   - Lifetimes (E2B `timeout`, Daytona auto-stop, auto-delete and TTL) are enforced by the gateway's reaper. They move to the core `Sandbox.set_lifetime` (feat/cua-e2b) when that merges.
   - Provider API keys map to Cua identities in `keys.json` (0600). A key sees only its own sandboxes.

### Environment a client gets

E2B:
- `E2B_API_URL` and `E2B_SANDBOX_URL`: `http://127.0.0.1:<api>`. The SDK verifies TLS for these with the OS store, so loopback HTTP is the secure and simple choice.
- `E2B_DOMAIN=e2b.localhost:<tls>`.
- `E2B_API_KEY`.
- `SSL_CERT_FILE`.

Daytona:
- `DAYTONA_API_URL=http://127.0.0.1:<api>/api`.
- `DAYTONA_API_KEY`.
- `SSL_CERT_FILE`.

Modal:
- `MODAL_SERVER_URL=http://127.0.0.1:<api>`. The client accepts plain `http` for loopback (and the JavaScript and Go libraries are told an `https` URL for the command router, then connect without TLS to loopback).
- `MODAL_TOKEN_ID` (`ak-` and a hash of the key) and `MODAL_TOKEN_SECRET` (the key).
- `SSL_CERT_FILE`, for the TLS names tunnels are served on.

`cua compat env --provider e2b|daytona|modal` prints them.

## Providers

- `e2b/`: the REST control plane (v1 and v2), envd's Connect-RPC process and filesystem services, `/files` (multipart, octet-stream, gzip, signed URLs) and `/health`.
- `daytona/`: the REST API, the toolbox (exec, code run, sessions with follow logs, files, bulk transfer, signed URLs, ports, metrics), preview and signed preview URLs, snapshots, and Socket.IO state events.
- `modal/`: a tonic gRPC server (`ModalClient`, 267 RPCs, and `TaskCommandRouter`, 23) generated from the pinned client's descriptors (`modal-proto/`), with our own handlers.
  - Sandboxes and their exec, stdio, stdin, filesystem and snapshot calls; Images built layer by layer in Spaces and cached by spec hash; Volumes, CloudBucketMounts, Secrets, Dicts, Queues, Apps and Environments.
  - Functions and Classes: calls and inputs are queued per Function; an autoscaler starts containers (Spaces from the Function's Image plus the `modal` client) that run Modal's own container entrypoint and call back with a task token.
  - `modal-proto/drift.py` diffs the descriptors of a newer client against the served set; the weekly workflow runs it.
- `maps/` lists every call of each provider with its status. Calls with no Cua equivalent answer with the provider's error and the reason.

## Rules it keeps

- **Isolation axiom:** every sandbox and every job is a Space (image + kind), never a host process. The gateway runs no user code.
- **Loopback by default.** Keys are required on every route, and envd and toolbox calls need the sandbox's access token or signature.
- **Layering:** compat → SDK → daemon/spacesd → vmm/images. Core features (job services, the hotspot, tunnels, lifetimes, fork) live in core and are consumed here.
