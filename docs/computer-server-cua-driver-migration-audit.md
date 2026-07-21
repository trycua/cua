# Migrating Cua surfaces from computer-server to Cua Driver

Status: implementation audit and proof of concept

## Question

Can Cua stop maintaining `cua-computer-server` by replacing it with the Python
`cua-driver` SDK, either behind the existing server protocol or directly in
the products that currently install the server?

## Preliminary answer

Use Cua Driver as the single GUI implementation, but do not replace the remote
computer-server boundary with the local SDK in sandbox and VM products yet.
The SDK and the server solve different halves of the current system:

- Cua Driver owns local, policy-aware, cross-platform GUI perception and input.
- computer-server owns a remotely reachable control plane for GUI, shell,
  files, PTYs, browser execution, authentication, and compatibility clients.

The recommended transition is to hollow out computer-server's duplicated GUI
code behind a Cua Driver adapter, migrate co-located client applications to the
SDK directly, and then split or replace the remaining remote control plane as
a separate product decision.

## Audit method

The inventory searches maintained source, package manifests, image definitions,
CI/release workflows, tests, and documentation for explicit server package
references and implicit protocol dependencies such as port 8000, `/cmd`,
`/ws`, `/status`, and PTY routes. Generated release history, dependency locks,
and recorded browser transcripts are excluded from product-surface counts.

## Surface inventory

The audit found six kinds of dependency. A reference is called **protocol**
when it does not import or install the package but assumes computer-server's
ports, routes, request shapes, or service name.

| Surface | Dependency today | Representative repository paths | Recommended disposition |
| --- | --- | --- | --- |
| Server implementation and package | About 14,000 lines of Python; publishes `cua-computer-server` and owns native automation plus the remote service | `libs/python/computer-server`, root `pyproject.toml`, `Makefile` | Keep temporarily as a compatibility gateway, but make Cua Driver its GUI engine and stop adding native GUI code here |
| Python Computer SDK | Protocol client for `/ws` and `/cmd`; also uses dedicated `/pty` routes and launches/provisions the server | `libs/python/computer/computer.py`, `computer/interface/generic.py`, `computer/pty.py`, `computer/providers/{cloud,docker,winsandbox}` | Keep the remote SDK abstraction; change its target only after a replacement guest gateway exists. A local-host provider can use Cua Driver directly |
| Python Sandbox SDK | Protocol transports for `/cmd`, `/ws`, `/status`, files, shell, and PTYs; runtimes reserve and forward API ports | `libs/python/cua-sandbox/cua_sandbox/{transport,runtime,interfaces,builder}`, `cua_sandbox/sandbox.py` | Do not point this at the local Cua Driver socket on the host: that would control the wrong desktop. Migrate to a new guest gateway or compatibility server |
| TypeScript Computer SDK | Protocol client that constructs `ws(s)://.../ws` on ports 8000/8443 | `libs/typescript/computer/src/interface/base.ts` | Same remote-boundary requirement as Python Computer; a separate local SDK consumer can import `@trycua/cua-driver` |
| CLIs and application UI | Probe `/status`, proxy `/cmd`, expose server MCP tools, or construct Computer/Sandbox clients | `libs/python/cua-cli/cua_cli/commands/{do,mcp,sandbox}.py`, `libs/typescript/cua-cli/src/commands/{sandbox,serve-mcp}.ts`, `libs/typescript/playground`, `libs/python/agent`, `libs/python/mcp-server` | Client-app commands follow the SDK/gateway migration. Agent integrations should invoke `cua-driver mcp` directly when co-located instead of importing a language MCP facade |
| Cua Bench and WinArena | RemoteComputer and runners issue server commands; Daytona publishes port 8000; WinArena images and evaluators rely on shell/files/window operations | `libs/cua-bench/cua_bench/{computers,runner,sessions}`, `libs/cua-bench/tasks/winarena_adapter` | Keep the remote gateway until benchmark setup/evaluation has replacements for shell, files, PTY, and platform-specific getters. GUI actions can move behind the adapter first |
| Fleet examples | Discover a `computer-server` guest service, health-check `/status`, and expose its MCP tools to agent SDKs | `libs/fleet/js-sdk/examples/{claude-agent-sdk,pi-agent}.mjs`, `libs/fleet/python-sdk/examples`, mini-SWE YAML profiles | Advertise `cua-driver mcp` as the agent GUI service and a distinct guest-control service for app/sandbox operations. Do not describe both as one SDK |
| XFCE image | Installs the Python server and starts port 8000 under Supervisor | `libs/xfce/{Dockerfile,Dockerfile.dev,src/supervisor,src/scripts}` | POC supports either compatibility server or direct driver. Direct mode is valid only for co-located clients until a remote gateway is added |
| Kasm image | Installs/updates the package and starts `python -m computer_server` | `libs/kasm/Dockerfile` | Migrate through the compatibility adapter first; later select driver plus guest gateway |
| QEMU Linux and Windows guests | Guest setup services install the package, open port 5000, start it, and outer entrypoints poll `/status` | `libs/qemu-docker/linux`, `libs/qemu-docker/windows` | The local SDK alone cannot replace these services because the host needs a remote readiness and command boundary |
| QEMU Android | Installs and starts computer-server on port 8000, although screenshot has a faster emulator gRPC path | `libs/qemu-docker/android`, Android handlers in computer-server | Exclude from the first driver migration. The current Cua Driver backend rejects Android; converge separately around emulator gRPC/ADB capabilities |
| Lume guests | Guest setup installs and launches the package; Sandbox/Computer clients connect over the VM network | `libs/lume/scripts/setup-cua.sh`, Python Lume runtime/provider paths | Use the adapter first. A final replacement requires a macOS guest gateway or another authenticated host-to-guest channel |
| Windows Sandbox | PowerShell bootstrap installs/starts the server and Python provider waits for it | `libs/python/computer/computer/providers/winsandbox` | Use the adapter once driver packaging and daemon lifecycle are certified in Windows Sandbox; keep the remote gateway |
| Tests, CI, and releases | Dedicated Python package CI/CD, shared Python CI, integration fixtures, image tests, and version/release catalog entries | `.github/workflows/*computer-server*`, release workflows, `tests`, package tests | Retire only after every image and remote SDK has moved; until then add adapter conformance and image matrix coverage |
| Documentation and historical content | User guidance and examples name the package or its endpoints | root `README.md`, `blog`, `docs`, changelog | Update along each migrated product surface; preserve historical posts/changelogs as history |

Two incidental groups are not product consumers: comments in Cua Driver's
macOS input code compare old computer-server key behavior, and `cua-auto` is a
terminal implementation used by computer-server. They are maintenance context,
not independent deployment surfaces.

## What computer-server owns

The server is not just an alternate implementation of `get_desktop_state`.
Its command registry exposes more than forty commands across these domains:

- desktop input, screenshot, cursor, and screen geometry;
- accessibility lookup and UI trees;
- clipboard, shell execution, and files;
- app/window lifecycle, geometry, activation, and wallpaper;
- diorama and Android-specific gestures.

Its network layer separately owns authenticated HTTP/WebSocket compatibility
and service discovery:

- `GET /status` and `GET /commands`;
- `POST /cmd` using a one-frame SSE response and `/ws` using command JSON;
- HTTP and WebSocket PTY lifecycle under `/pty`;
- `/responses` for an in-guest agent proxy and `/playwright_exec`;
- streamable HTTP MCP at `/mcp`.

The remote layer also emits VM-session telemetry and can gate routes on cloud
container availability. Those concerns belong to the compatibility/gateway
boundary, not the local GUI driver.

These routes explain why replacing a package in an image is not sufficient.
Most Cua consumers run outside the guest and depend on this control plane.

## Capability comparison

The stable Rust contract currently generates fourteen typed Python and
TypeScript operations: session start/end/state/escalation, desktop state,
screen/cursor state, click/move/drag/scroll, and text/key/hotkey input. The SDK
also has a generic `call_tool` escape hatch into the complete live registry.
On the audited macOS build that registry advertised 48 tools, adding browser,
window/app, recording, cursor-overlay, permission, and diagnostic features.
Those extra tools are useful but are not a portable replacement for all server
commands.

| Capability | Cua Driver SDK/daemon | computer-server | Migration consequence |
| --- | --- | --- | --- |
| Whole-desktop screenshot and coordinates | Typed and portable; session-scoped capture policy | Native implementation with legacy result shape | Direct overlap; adapter maps driver images into server PNG/JPEG responses |
| Click, move, drag, scroll, type, key, hotkey | Typed and portable | Native per-OS handlers | Direct overlap, subject to semantic conformance tests |
| Held mouse/key state | Not in the typed contract | `mouse_down/up`, `key_down/up` | Adapter must fall back or driver must add canonical operations |
| Session capture scope | `auto`, `window`, `desktop`, with explicit escalation | No equivalent policy model | Driver is the desired source of truth; compatibility defaults to `desktop` |
| Accessibility and window-aware state | Available in the live tool registry; platform behavior is richer but not all calls are in the portable typed subset | Server has its own tree/find/window shapes | Do not silently substitute until schemas and cross-platform parity are certified |
| App/window lifecycle | Several live tools, currently platform-skewed | Cross-platform generic/native handler surface | Add portable contracts or keep gateway fallbacks |
| Browser control | Structured driver browser tools bound to a target tab | Arbitrary `/playwright_exec` plus agent proxy | Different security and semantic model; migrate callers deliberately |
| Recording and diagnostics | Rich live driver tools | Limited/other mechanisms | Prefer driver implementations |
| Clipboard | No canonical driver operation | Get/set clipboard | Retain fallback or add a reviewed driver contract |
| Shell, files, and PTY | Not a driver responsibility today | Full command/file operations and interactive PTY | Requires a separate guest-control service or existing SSH/SFTP/other transports |
| Wallpaper and diorama | Not canonical driver operations | Server commands | Keep or retire per product need |
| Remote TCP, auth, health, discovery | Local socket/pipe only | HTTP/WebSocket, headers, cloud auth, status | The key blocker to direct replacement in VM/container products |
| VNC backend | No | Yes | Preserve where local OS integration is unavailable |
| Android | Not supported by this backend | ADB plus EmulatorController gRPC paths | Separate migration track |
| Agent MCP | Bundled `cua-driver mcp` proxy | Server `/mcp` | Prefer driver MCP for co-located agents; this is separate from app SDK usage |

The local-only boundary is intentional and valuable: SDK calls control the OS
where the daemon runs. A host application cannot import `cua-driver` and expect
it to control a desktop inside a remote guest.

## Proof of concept A: driver-backed compatibility server

The branch adds `CUA_BACKEND=cua-driver` and an optional
`cua-computer-server[driver]` dependency. The compatibility handler connects to
the local daemon through the published Python SDK, lazily starts a session, and
routes the portable overlapping GUI operations through `call_tool`. This use of
the generic method is deliberate: the SDK documents it as the escape hatch for
application-owned server adapters, while normal app code should prefer the
typed methods.

The adapter:

- defaults to one desktop-scope compatibility session;
- deliberately fixes the session to `desktop` because the legacy protocol has
  neither window target identity nor the agent-ladder escalation signal;
- can select a daemon socket and compatibility session ID;
- moves synchronous UniFFI calls to worker threads so FastAPI's event loop is
  not blocked;
- translates screenshots, geometry, and errors into existing server shapes;
- delegates held buttons/keys, clipboard, and shell to the native handler;
- leaves accessibility, files, desktop, diorama, and window handlers unchanged;
- also reroutes computer-server's existing `/mcp` GUI tools because that surface
  shares the same handlers, while `cua-driver mcp` remains the preferred direct
  agent boundary;
- is opt-in, so the default `native` and existing `vnc` behavior is preserved.

This POC proves the safest first migration: one Rust GUI engine can sit behind
the existing remote contract without forcing every client, image, and benchmark
to move atomically.

Known limitations are explicit rather than hidden:

- the compatibility identity is process-wide rather than one driver session
  per authenticated remote caller;
- the handler relies on the daemon's idle cleanup because computer-server has
  no handler lifecycle hook that ends the session on shutdown;
- a legacy multi-point drag path is represented by endpoints plus a step count;
- legacy scroll deltas are mapped to driver direction/line amounts and clamped;
- accessibility/window behavior still comes from Python, so this POC removes
  duplicated automation code from the hot path but not the whole package.

## Proof of concept B: XFCE without computer-server

The XFCE release Dockerfile now has a default-preserving build argument:

```text
CUA_GUI_RUNTIME=computer-server  # published behavior
CUA_GUI_RUNTIME=cua-driver       # incompatible direct-driver POC
```

The second variant installs only `cua-driver`, runs its local daemon under
Supervisor, and includes a Python verifier that uses the generated typed SDK to
start a desktop session, read the screen size, capture the desktop, and end the
session. It also verifies an input call without changing UI state by moving the
cursor to its current position. The Linux daemon explicitly uses its default
`standard` authorization mode; the macOS-only permissions gate is irrelevant in
this image. PyPI 0.10.0 was verified to publish both Linux x86_64 and aarch64
wheels, matching the XFCE image's architecture matrix. The selector is limited
to the release Dockerfile; `Dockerfile.dev` intentionally remains a
computer-server development image for this POC.

This POC also proves the failure mode: it deliberately has no port 8000
service. Code running inside the container can import `cua_driver`, and a
co-located agent can run `cua-driver mcp`, but current Computer, Sandbox, Bench,
CLI, Playground, and Fleet clients cannot reach it from outside. Replacing the
image package without replacing the control plane therefore breaks the product
contract even though GUI automation itself works locally.

## Decision

Adopt POC A as the migration seam. Do not ship POC B as the general replacement
for remote Cua environments yet.

The target architecture should have three intentionally separate boundaries:

1. **Cua as an application SDK:** co-located Python and TypeScript applications
   import `cua-driver` / `@trycua/cua-driver` and use generated typed methods.
2. **Cua as an agent tool:** co-located agents launch `cua-driver mcp` (or use
   its CLI). MCP is already runtime-neutral, so Cua should not generate language
   MCP clients.
3. **Cua as a remote sandbox/VM:** host applications use Computer/Sandbox SDKs
   against a small authenticated guest-control service. That service delegates
   GUI work to the local Cua Driver daemon and owns only remote lifecycle,
   health, shell/files/PTY, and compatibility concerns.

Calling the third boundary `cua-driver` would blur a useful trust and topology
distinction. It should be treated as a guest gateway even if the first
implementation remains the hollowed-out Python computer-server.

## Recommended migration plan

### Phase 1 — certify the adapter

- Land the opt-in backend without changing default images.
- Add Linux X11, macOS, and Windows protocol-conformance tests that run the same
  legacy command vectors against `native` and `cua-driver` backends.
- Define acceptable deltas for coordinate scaling, scroll units, drag paths,
  image formats, locked desktops, and error codes.
- Decide whether to add canonical driver contracts for held key/button state and
  clipboard or explicitly keep those outside the driver.
- Map a remote connection or agent run to a driver session instead of sharing a
  process-global compatibility session.

### Phase 2 — remove duplicate GUI implementations

- Make driver-backed automation the default in XFCE, Kasm, QEMU Linux/Windows,
  Lume, and Windows Sandbox only after their platform suites pass.
- Route accessibility/window actions through portable typed driver contracts as
  they become certified; stop modifying the equivalent Python handlers.
- Keep VNC and Android on explicit alternative tracks rather than pretending
  they have driver parity.
- Move co-located app code to direct SDK imports and co-located agents to
  `cua-driver mcp`.

### Phase 3 — define and build the guest gateway

- Inventory the exact remote operations that product surfaces still need after
  GUI migration: health/discovery, authentication, shell, files, PTY, and any
  benchmark-only operations.
- Design one versioned remote contract for those operations. Reusing the current
  wire protocol can make rollout non-breaking; a new protocol needs dual-stack
  clients and images during migration.
- Implement the gateway in the language that minimizes lifecycle and packaging
  cost. Rust is attractive once the contract is stable, but changing language
  does not remove the need for a remote service.
- Keep GUI calls as local SDK calls from the gateway to Cua Driver. Do not expose
  the daemon socket directly over the network.

### Phase 4 — migrate by surface and retire

- Move images one at a time with health, GUI, shell/file, PTY, auth, and agent
  smoke tests; maintain compatibility routes while old SDKs are supported.
- Update Computer/Sandbox SDK transports, then CLI/Playground/Bench/Fleet
  consumers, followed by docs and examples.
- Remove `/mcp` from the guest gateway once all agent surfaces use
  `cua-driver mcp` directly.
- Retire the Python package, its native GUI handlers, release workflows, and
  port assumptions only when repository search and end-to-end matrices show no
  maintained consumer remains.

## Validation evidence

- Claude Code Fable was requested for the architecture audit, but its provider
  rejected the run before inference because that model's usage allowance was
  exhausted. A read-only Claude Code Opus review was used as the available
  fallback. It found no blocking correctness or security issue and agreed that
  the compatibility adapter is the defensible migration seam. Its findings led
  to the executable verifier, real input-path smoke, value-normalization tests,
  Linux authorization clarification, and expanded remote-boundary inventory in
  this document.
- All computer-server tests: 60 passed.
- New adapter tests cover session declaration, pointer translation, implicit
  cursor/scroll coordinates, PNG/JPEG compatibility, native fallbacks, and
  driver rejection shapes, legacy value normalization, plus CLI backend
  selection: 7 passed as part of the suite.
- The computer-server wheel built successfully and its metadata contains the
  `driver` extra with `cua-driver>=0.10.0,<0.11.0`.
- The local Python environment imported the generated UniFFI SDK and constructed
  a client from the repository package. No daemon was running, so a live desktop
  action was not claimed.
- XFCE launcher syntax, verifier Python compilation, executable installation
  instruction, formatting/lint, lockfile consistency, and patch whitespace were
  checked.
- PyPI's release metadata confirmed 0.10.0 wheels for macOS universal2, Linux
  x86_64/aarch64, and Windows x86_64/arm64.
- This workstation has no Docker, Podman, Buildah, or Hadolint, so the XFCE image
  was not built or booted locally. A multi-architecture image build plus the
  in-container verifier remains a required CI/Docker-host gate before POC B can
  be described as runtime-proven.
