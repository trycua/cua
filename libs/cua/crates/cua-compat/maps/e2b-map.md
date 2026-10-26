# E2B wire-compat map for Cua Spaces (cua-compat gateway)

Date: 2026-10-19. Clients examined: `e2b` 2.42.0 (the pinned version) and 2.56.0 (latest on PyPI today), both read from the installed wheels. Server-side contracts: the public REST spec (`spec/openapi.yml` in e2b-dev/infra) as reflected in the SDK's generated client, and the envd `process` / `filesystem` protos embedded in the SDK (`e2b/envd/*_pb.py`). Treated as data only; nothing copied.

**Status key:**
- **clean**: the gateway implements the call over the Cua SDK with the same observable behaviour.
- **partial**: implemented, with a stated difference.
- **no mapping**: rejected with E2B's own error shape, and the reason is given.

## 1. How the SDK picks endpoints (connection_config.py:120-140, 234-269 in 2.42)

- **API:** `api_url` = `E2B_API_URL`, else `http://localhost:3000` in debug, else `https://api.<E2B_DOMAIN>`. Plain `http://` is accepted.
- **envd (in-sandbox agent):** `E2B_SANDBOX_URL` wins for every sandbox. Otherwise debug gives `http://localhost:49983`, the E2B domains give `https://sandbox.<domain>`, and anything else gives `https://49983-<id>.<domain>`.
- **get_host(port):** `<port>-<id>.<sandbox domain>`. In debug mode it is `localhost:<port>`, which is useless with more than one sandbox. The sandbox domain comes from the create response's `domain`, so the **server decides it**.
- **envd headers:** every envd request carries `E2b-Sandbox-Id`, `E2b-Sandbox-Port: 49983`, and `X-Access-Token` when the create response returned `envdAccessToken`. The user is sent as `Authorization: Basic base64("<user>:")`. So one envd URL can serve every sandbox, routed by header.
- **API key:** sent as `X-API-KEY`. 2.42 requires the `e2b_<hex>` format unless `E2B_VALIDATE_API_KEY=false`; 2.56 no longer validates it.
- **Transports:**
  - REST and envd RPC use pyqwest (reqwest/hyper). 2.56 defaults to HTTP/2, which means h2c prior-knowledge on `http://`.
  - File transfer uses httpx on the same pool.
  - **pyqwest verifies TLS with the OS verifier** (rustls-platform-verifier: Keychain on macOS) and **ignores `SSL_CERT_FILE`**, as tested. httpx honours `SSL_CERT_FILE`.
- **envd RPC wire:** Connect protocol with the **JSON codec**:
  - unary: `POST /<pkg>.<Svc>/<Method>`, JSON body;
  - server streams: `application/connect+json` envelopes;
  - client stream: `StreamInput`, envelopes.
  - int64 is a JSON string, bytes are base64, enums are names, fields are lowerCamel. Errors are a Connect JSON error, or a plain HTTP status that the SDK maps (404 → NotFound, 502 → "sandbox timeout", …).

**Gateway consequence:** the zero-diff setup is `E2B_API_URL=http://127.0.0.1:<p>`, `E2B_SANDBOX_URL=http://127.0.0.1:<p>`, and `E2B_API_KEY=e2b_<gateway key>`.
- These are plain HTTP on loopback, so pyqwest never needs our CA.
- `get_host(port)` returns `<port>-<id>.e2b.localhost:<tls port>`. macOS and glibc resolve `*.localhost` to loopback (tested on both Macs).
- The gateway serves that host over TLS with a wildcard leaf signed by a local CA. Callers that do `https://{get_host(...)}` (httpx, aiohttp, urllib) trust it through `SSL_CERT_FILE`. That file is a bundle of the OS roots plus our CA, so an RL trainer and other public HTTPS calls keep working.

## 2. Control plane (REST, `X-API-KEY`)

| SDK call | Route | Cua mapping | Status |
|---|---|---|---|
| `Sandbox.create(template=None/"base")` | `POST /sandboxes` (2.42), `POST /v2/sandboxes` (2.56) | `Spaces.create(image=<default linux>, kind=vm, on=<cfg>)` | clean |
| `create(template=<snapshot id or name>)` | same | A snapshot this machine holds and no `cua.on` (or `cua.on=local`): `Sandboxes.fork(checkpoint)`, the item-1 CoW fork. Otherwise `Spaces.create(image="checkpoint:<name>", on=<cua.on>)`: the core starts the fork on the machine of yours that holds the snapshot, or where `cua.on` says (`host:<machine>`, `auto`, `local`) after a verified copy (snapshot sync). Template resolution sees snapshots on every machine of the pool (`Sandboxes.checkpoints()`, `Checkpoint.host`). No room on any machine: the create waits (bounded) like an image create | clean |
| `create(template=<template alias>)` | same | gateway template table (`templates.json`: alias → image or checkpoint) | clean |
| `create(timeout=…)` | `timeout` field | gateway lifetime reaper. The deadline is persisted, so a gateway restart keeps it. On expiry the sandbox is deleted, and later calls get 404 / "sandbox timeout", like E2B | clean |
| `create(metadata=, envs=)` | `metadata`, `envVars` | metadata kept by the gateway (list/get_info filters). envVars become the default env of every process | clean |
| `create(allow_internet_access=False)` | `allow_internet_access` | `network="none"` on QEMU VMs. Other kinds reject it, and the gateway returns 400 | partial |
| `create(secure=True)` | `secure` | gateway mints `envdAccessToken` and enforces `X-Access-Token` on envd routes | clean |
| `network={allow_out/deny_out/...}`, `mcp=`, `iam=`, `volume_mounts=`, `lifecycle.auto_pause/auto_resume` | fields | no Cua equivalent today (egress rules per host, E2B's MCP gateway, IAM, E2B volumes) | no mapping: 400 "not supported by this gateway" when set |
| `Sandbox.connect(id)` / `AsyncSandbox.connect` | `POST /sandboxes/{id}/connect`, `/v2/...` | refresh timeout. A suspended sandbox is resumed (`Sandbox.resume`) | clean |
| `kill()` / `Sandbox.kill(id)` | `DELETE /sandboxes/{id}` | `Spaces.delete` / `Sandbox.delete` (204, 404 if unknown) | clean |
| `set_timeout(t)` | `POST /sandboxes/{id}/timeout` | reaper deadline | clean |
| `get_info()` | `GET /sandboxes/{id}` | gateway record + `Sandboxes.get` (state, cpu, memory) | clean |
| `Sandbox.list(query)` paginator | `GET /v2/sandboxes?state&metadata&limit&nextToken` (`x-next-token` header) | gateway records (sandboxes it created) | clean |
| `is_running()` | envd `GET /health` | spacesd health | clean |
| `pause()` / `beta_pause()` | `POST /sandboxes/{id}/pause` | `Sandbox.suspend` (QEMU: RAM kept). 409 if already paused | clean |
| `resume` (via connect) | `POST /sandboxes/{id}/resume`, connect | `Sandbox.resume` | clean |
| `create_snapshot(name=)` | `POST /sandboxes/{id}/snapshots` → `{snapshotID, names}` | `Sandboxes.checkpoint(sandbox, name)`. A sandbox on another machine of the pool is checkpointed on that machine (`HostSnapshotsService.CreateHostSnapshot`), which keeps the snapshot | clean |
| `list_snapshots()` | `GET /snapshots` | `Sandboxes.checkpoints()`: this machine's, then those your other machines hold, each name once | clean |
| `delete_snapshot(id)` | `DELETE /templates/{id}` | `Sandboxes.delete_checkpoint`: here and on every machine of the pool that holds a copy (`HostSnapshotsService.DeleteHostSnapshot`); 404 when none does | clean |
| `fork(count=n)` | `POST /sandboxes/{id}/fork` → list of `{sandbox}` / `{error}` | checkpoint the live sandbox, then n × `Sandboxes.fork` (item 4 batch fork when merged) | clean |
| `get_metrics()` | `GET /sandboxes/{id}/metrics` | spacesd `System.Metrics` (cpu %, mem, disk) as one sample | clean |
| `get_logs` / `/v2/.../logs` | | E2B's per-sandbox log stream of envd and processes; Cua has no per-Space log API | no mapping: 404 |
| `update_network()` | `PUT /sandboxes/{id}/network` | no runtime egress rules | no mapping: 400 |
| Templates: `Template.build`, `e2b template build`, aliases, tags, builds, file hashes | `/templates*`, `/v2/templates`, `/v3/templates`, `/templates/tags` | Cua builds images with `cua images build` (Dockerfile → VM disk). E2B's build protocol (layered steps, file upload by hash, build logs) is a different system | no mapping: 400 "build with cua images build; map the alias in templates.json". Template **use** is clean (row 3) |
| Volumes (`e2b.Volume`), secrets (`e2b.Secret`, 2.56) | `/volumes*`, `/secrets*` | Cua Volume is per-user storage with no E2B-style volume objects. No secret store API | no mapping: 404 |

## 3. Data plane (envd)

| SDK call | envd RPC / route | Cua spacesd mapping | Status |
|---|---|---|---|
| `commands.run(cmd)` fg | `process.Process/Start` (stream), `/bin/bash -l -c cmd` | `ProcessService.StartProcess`, field by field. Exit status, signal and `error` are formatted the way envd does (`exit status N`, `signal: killed`) | clean |
| `run(user=, cwd=, envs=, timeout=)` | Basic auth user; `ProcessConfig.cwd/envs`; deadline is client-side | `ProcessConfig.user/cwd/env`. The E2B default user `user` maps to a configurable guest user (default `root` when the image lacks `user`) | clean |
| `run(background=True)` → handle, `wait`, `disconnect`, `kill`, `on_stdout/on_stderr` | Start stream; drop stream; `SendSignal` | same. Processes outlive a dropped stream (spacesd keeps them) | clean |
| `commands.list()` | `Process/List` | `ListProcesses` | clean |
| `commands.kill(pid)` | `Process/SendSignal(SIGKILL)`; false on NotFound | `SignalProcess` | clean |
| `commands.send_stdin(pid, data)`, `close_stdin` | `SendInput`, `CloseStdin` | same | clean |
| `commands.connect(pid)` | `Process/Connect` (stream) | `ConnectProcess` | clean |
| `run(stdin=True)` + `StreamInput` | client stream | `StreamInput` | clean |
| `pty.create/resize/send_stdin/kill` | `Start` with `pty`, `Update`, `SendInput{pty}` | `PtyConfig`, `ResizePty` | clean |
| `files.read(text/bytes/stream)` | `GET /files?path&username` | `Filesystem.ReadFile` (chunked), streamed straight through | clean |
| `files.write(path, data)` / `write_files` (multipart and octet-stream) | `POST /files` → `[{name,type,path}]` | `WriteFile` with create_parents, then chown for non-root users | clean |
| `files.list(depth)`, `get_info`, `exists`, `make_dir`, `rename`, `remove` | `ListDir`, `Stat`, `MakeDir`, `Move`, `Remove` | same RPCs; EntryInfo fields mapped (type, size, mode, permissions string, owner, group, mtime, symlink) | clean |
| `files.watch_dir` (stream) and `CreateWatcher` / `GetWatcherEvents` / `RemoveWatcher` | | `WatchDir`, `CreateWatcher`, … (same shape) | clean |
| `get_host(port)` + HTTP(S)/WebSocket to it | wildcard `<port>-<id>.<domain>` | gateway TLS router → `Sandbox.forward(port)` (loopback, or the item-5 LAN endpoint) | clean |
| `files.write(metadata=…)`, EntryInfo `metadata` (envd ≥ 0.6.2 xattr metadata) | `X-Metadata-*` headers | spacesd has no xattr API | partial: the gateway reports envd `0.6.1`, so the SDK raises its own "metadata not supported by this sandbox" error |
| `git.*` | wrappers over `commands.run` | runs `git` in the guest | clean (nothing to do) |
| `sandbox.mcp*` (E2B MCP gateway) | `mcp` create field | | no mapping (see row above) |

## 4. Wire-compat feasibility verdict

**Feasible and clean for the whole sandbox runtime surface, with zero client code changes.** Evidence:
- The SDK accepts http API and envd URLs from env vars.
- envd requests are routable by header.
- The sandbox domain is server-chosen.
- `*.localhost` resolves without DNS setup.
- The envd protos are close to 1:1 with Cua's `cua.env.v1` process and filesystem services.

What does not map is E2B's *platform* surface: template building, volumes, secrets, logs, network rules, IAM and the MCP gateway. Each gets a typed error.

**Drift risk** is moderate: 2.42 → 2.56 moved create/connect to `/v2`, switched to HTTP/2, and added secrets. The weekly scheduled run against the latest SDK catches this.

## 5. Licensing

- `e2b` Python SDK: MIT (`License-Expression: MIT` in both wheels).
- e2b-dev/infra (API spec, envd protos): Apache-2.0.
- Both are permissive, so matching the public API and wire shapes is allowed. The gateway is our own implementation: hand-written serde types and Connect framing, no generated or copied E2B code.
- The E2B SDK is used unmodified, only as a test dependency.
- Say "compatible with the E2B SDK"; do not use the E2B name or marks as a product name.
