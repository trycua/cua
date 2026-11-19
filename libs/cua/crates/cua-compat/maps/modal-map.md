# Modal -> Cua Spaces compatibility map (analysis of the client)

Date: 2026-10-19
Client examined: `modal` 1.6.2 (PyPI wheel, py3-none-any), unpacked at
`<scratch>`. Sources: `modal/sandbox/_sandbox.py`,
`_filesystem.py`, `_task_command_router_client.py`, `client.py`, `config.py`,
`_utils/grpc_utils.py`, `modal_proto/*_pb2*.py(i)`, wheel METADATA/LICENSE. Third-party code read as data only.

Note: 1.6.2 has no `sb.open`, `sb.ls`, `sb.mkdir`, `sb.rm`. The file API is `sb.filesystem.*`
(`read_bytes/read_text/write_bytes/write_text/list_files/make_directory/remove/stat/watch/copy_from_local/copy_to_local`).
`ContainerFilesystemExec` is gone from the client; the file helpers are built on exec.

## Call map

The Status column is the analysis made before the gateway existed (what a mapping would need). The current status of every public symbol, with its test, is in `modal-coverage.md`.

| Modal call | Cua equivalent | Status | Reason |
|---|---|---|---|
| `Sandbox.create(image=, app=)` | create_space(image) | partial | Cua images are VM images; Modal `Image` is a server-built layer graph (`from_registry`, `debian_slim`, `.pip_install`). Needs an image-build emulation. |
| `timeout=` / `idle_timeout=` | Space lifetime / idle stop | clean | Plain TTL fields. |
| `cpu=`, `memory=` (request, limit tuples) | VM vCPU / RAM at create | partial | VMs have fixed size, no request/limit split; also `gpu`, `region`, `ephemeral_disk`, `cloud`. |
| `encrypted_ports`, `unencrypted_ports` | port forwards | partial | Cua forwards exist; TLS-terminated vs raw split needs a gateway. |
| `block_network`, `cidr_allowlist`, outbound policy | Space network policy | partial | Only if the VM layer has egress filtering. |
| `volumes={path: Volume}` | Cua Volume mount | partial | Modal Volume is its own object with its own RPCs (commit/reload); only the mount semantics map. `CloudBucketMount` not mapped. |
| `secrets=[Secret]` | env injection at create/exec | clean | Server stores secrets by id; resolved to env vars. Needs Secret RPCs. |
| `Sandbox.exec(*argv, ...)` | space_bash / exec | clean | Argv, env, workdir, timeout, pty. Runs on a separate "command router" service (see below). |
| `proc.stdout/stderr` streams | exec output stream | clean | Server-streaming `TaskExecStdioRead`. |
| `proc.stdin.write/drain/write_eof` | exec stdin | partial | Needs stdin-write and stdin-status RPCs; Cua exec may lack open stdin. |
| `proc.wait()`, `returncode`, `poll()` | exec exit status | clean | `TaskExecWait`/`TaskExecPoll`. |
| `sb.filesystem.read/write/list/stat/remove/mkdir` | Cua file read/write/list | partial | Client does NOT use file RPCs: it execs a binary at `/__modal/.bin/modal-sandbox-fs-tools` inside the guest. The guest image must ship a compatible tool, or the shim must answer those execs. |
| `filesystem.watch` | none | no mapping | Same guest tool with inotify-like protocol. |
| `sb.snapshot_filesystem()` | checkpoint disk -> Image | partial | Returns a Modal `Image` id usable in later `Sandbox.create(image=)`; needs Cua snapshot-as-image registry. |
| `snapshot_directory` / `mount_image` / `unmount_image` | none | no mapping | Mounting a snapshot into a live sandbox; no Cua concept. |
| memory snapshot: `_experimental_snapshot()` + `_experimental_from_snapshot()` | checkpoint/fork snapshot | partial | Good concept match (Cua has full-VM snapshots). API is underscore-experimental in 1.6.2; V2 path goes through the command router. |
| `sb.tunnels()` | port forward list | clean | Returns host/port per exposed port; Cua forward URL maps directly. |
| `sb.terminate()` | delete_space | clean | |
| `sb.wait()` | wait for exit | clean | Needs exit semantics (Space has no "program exit"; use the entrypoint process). |
| `sb.poll()` | status | clean | |
| `Sandbox.from_id(id)` | get space by id | clean | Id prefix `sb-`; any string works if the server mints it. |
| `Sandbox.from_name` | named Space | clean | Name -> id lookup. |
| `Sandbox.list(tags=)` / `set_tags` / `get_tags` | list_spaces + metadata | clean | Requires tag storage on Spaces. |
| `wait_until_ready(probe)`, `create_connect_token` | readiness / auth token | partial | Probe is TCP or exec; connect token is Modal-specific. |
| suspend/resume | sandbox_suspend | no mapping | No Modal client call for suspend (only snapshot/restore). Cua extra. |
| `Sandbox.create` gating calls: `App.lookup/create`, `Image` objects, `Secret`, `Volume`, `Environment` | account/app registry | partial | Client creates an App (`AppGetOrCreate`/`AppCreate`) and resolves Image/Secret/Volume objects before `SandboxCreate`; the server must fake all of them. |

## Wire-compat: built

Status: `cua run --compat modal` and `cua compat serve --provider modal` serve the Modal client's gRPC API. The coverage of every public symbol of the pinned client is in `maps/modal-coverage.md`; this file keeps the analysis of the client that the implementation was built from.

The earlier verdict here ("feasible but fragile, not recommended") was superseded: the descriptors recovered from the wheel make the stubs reproducible (`modal-proto/extract_descriptors.py`), a drift guard catches client changes (`modal-proto/drift.py`, weekly), and the hard parts turned out to be tractable:

- The server URL is `MODAL_SERVER_URL=http://127.0.0.1:<port>`: plain h2c, so no certificate work on the control plane. Token id and secret are headers (`x-modal-token-id`, `x-modal-token-secret`); the gateway checks them against its key file.
- 1.6.2 defaults to the V2 Sandbox RPCs (`SandboxCreateV2`, stdio through the command router, `x-modal-auth-token` from `AuthTokenGet`); the V1 RPCs are served too. The command router (`TaskCommandRouter`) is served on the same port with its JWT flow; the Python client takes `http` for a loopback router, and the JavaScript and Go libraries are given an `https` URL and connect without TLS to loopback.
- Image builds are layer by layer in Spaces (Dockerfile commands from `ImageGetOrCreate`), each layer a Cua checkpoint cached by a hash of the spec and its parents.
- The guest-side fs tool (`/__modal/.bin/modal-sandbox-fs-tools`) is answered by the gateway: the client's execs of the tool are recognised and served through cua-spacesd with the tool's JSON stderr errors.
- Functions: the client reaches `FunctionCreate`, `FunctionMap`, `FunctionPutInputs`, `FunctionGetOutputs` and friends; containers call `ContainerHello`, `FunctionGetInputs`, `FunctionPutOutputs`, `FunctionCallPutDataOut`, `ContainerHeartbeat`. The input-plane RPCs (`MapStartOrContinue`, `Attempt*`) are used only when a Function's metadata names an input-plane URL; the gateway names none.
- The `.proto` sources are not in the wheel; the descriptors are in the generated modules and are extracted from there.

## Licensing

- `modal` client: Apache-2.0 (`License-Expression: Apache-2.0`, LICENSE file in `.dist-info/licenses`).
- `modal_proto` is shipped in the same wheel under the same Apache-2.0 terms, but only as generated Python; no `.proto` sources.
- Apache-2.0 permits implementing a compatible server and using the generated stubs (keep notice and
  state changes if redistributing the files). No patent or trademark grant: avoid the Modal name/logo in product naming; describe as "compatible with the Modal Python client" only after counsel review. Clean-room rule: write our own server from observed wire behavior; do not copy client code.
- Their server and the hosted API are separate and under Modal terms of service; this work does not touch them.
