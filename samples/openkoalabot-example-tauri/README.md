# OpenKoalaBots (Rust + Tauri)

A desktop app where you hire named Bots, give each one its own computer (a
Space), and watch it work beside the thread. Built on the cua SDK's Rust
crates with a Tauri 2 shell and a React UI.

```sh
npx degit trycua/cua/samples/openkoalabot-example-tauri my-app
cd my-app && pnpm install
pnpm tauri dev
```

> The cua crates and the image list are not published yet, so a degit copy
> does not build on its own today. Until they are, develop inside the cua
> repo (below).

## Developing inside the cua repo

This example depends on the cua SDK by path, as a local workspace override:

| Dependency | Where it comes from today |
|---|---|
| `cua-spaces`, `cua-fleet`, `cua-sandbox-core`, `cua-daemon` | `../../../libs/cua/crates/*` (path dependencies in `src-tauri/Cargo.toml`) |
| The sandbox image list | `../../libs/images/sandbox-images.json`, imported by the UI (`ui/src/images.ts`) and compiled into the core (`src-tauri/core/src/plan.rs`) |

None of these are on crates.io or npm yet. Once they are, the path
dependencies become version dependencies and the degit quick start above
works as is.

```sh
cd samples/openkoalabot-example-tauri
pnpm install --frozen-lockfile
pnpm tauri dev          # Vite on :1430 plus the Rust shell
```

## What is in it

| Path | What |
|---|---|
| `src-tauri/core` (`openkoalabot-example-core`) | The app logic, headless, on `cua-spaces`: add, create and delete a Space, the desktop and per-window streams (media tickets), the window list, one agent thread per Bot, SHA-256 verified file drops, approval-gated session teleport (it ships with Cua Spaces; see below), presence, and the New Space plan (`plan.rs`). Also the `openkoalabot-example-scenario` runner. |
| `src-tauri/app` (`openkoalabot-example-tauri`) | The Tauri shell. Every command is a one-liner over the core. Local Spaces run on the SDK's own runtime (`cua_daemon::local::VmmLocal`: containers, QEMU, Lume). |
| `ui/` | The webview (Vite + React): the Bots roster, one thread per Bot with step, status, approval and file cards, the docked composer, the Computer panel (live desktop over media wire v2, decoded with WebCodecs, picture in picture, window list, drop zone), the New Space wizard and the teleport approval sheet. |
| `ui/src/assets` | The koala artwork (copied from `samples/openkoalabots-assets`). |

The UI follows the system's light or dark mode.

## Spaces

**New Space** opens a four-step wizard (System, Resources, Options,
Summary). The image dropdown lists the `published` entries of the shared
image list, and each entry names the engine it runs on in each location, so
the wizard never offers a combination the image cannot run. Create makes one
SDK call, `Spaces::create(SpaceCreate { on, image, kind, runtime, name, .. })`:

| Where | `on` | `kind` / `runtime` |
|---|---|---|
| This machine | `local` | container / `auto` (gVisor when available), vm / `qemu`, vm / `lume`; plus `cpus`, `memory_mb` |
| Cua Cloud | `cloud` | container / `gvisor`, vm / `kubevirt`. Needs `cua auth login` (or `CUA_CLIENT_ID`/`CUA_CLIENT_SECRET`); metered until deleted. |

**Add by address** (in the wizard footer or the empty state) registers a
machine that already runs cua-spacesd, for example a local container:

```sh
TOKEN=$(openssl rand -hex 16)
docker run -d --name cua-e2e-openkoalabots --runtime=runsc --memory=4g -e CUA_ENV_TOKEN=$TOKEN \
  -p 127.0.0.1:3211:3211 cua-e2e-local/linux:docker-local-arm64
# then add 127.0.0.1:3211 with $TOKEN
```

**Delete…** is a separate control that always asks first: a Space the app
created (in Cua Cloud or on this machine) is deleted with its sandbox; a Space
added by address is only removed from the list.

## Teleport

Teleport ships with Cua Spaces (source-available), not with the MIT SDK
this sample uses.

- **Session teleport** (the approval sheet) calls the Spaces tools
  `teleport_manifest` and `teleport_app` (`openkoalabot_example_core::teleport`).
  The app runs the Spaces runtime in process with no teleport extension, so
  the runtime refuses with `HostCapabilityMissing` and the sheet shows
  "Session teleport ships with Cua Spaces (source-available)."
- **App teleport** (the app picker, dropped app bundles and window drags) is
  not part of this sample. The drop zone's **Teleport an app…** button and a
  dropped app bundle show "App teleport ships with Cua Spaces
  (source-available)." and send nothing.

The Computer panel has one drop zone, the SDK's `<cua-drop-zone>`. Dropped
files upload (SHA-256 verified); **Send file…** picks files to upload.

## Picture in picture

The PiP button on the stream pops the desktop out; each row of the window
list pops out that one window. Each is a small always-on-top window
(`open_pip`) with its own media session, closed with it. Labels and routes
come from `@trycua/cua/spaces/pip` (`pipWindowLabel`, `encodePipRoute`).

## Host safety

The app keeps its Spaces registry and local sandbox state in its own data
directory (never `~/.cua`). Tests and the scenario use temp registries.

`OPENKOALABOTS_WALKTHROUGH=<file.json>` runs a scripted demo through the same
commands a person uses (add Spaces, hire Bots, send messages, open a sheet);
see the `Walkthrough` type in `ui/src/api.ts`.

## Tests

```sh
pnpm test                    # vitest: 57 tests (image dropdown, wizard, thread cards, Space section, media wire, PiP, drop zone, error messages)
cd src-tauri
cargo test -p openkoalabot-example-core -- --test-threads=4   # 34 unit + 8 hermetic + 4 plan
cargo build -p openkoalabot-example-tauri --features custom-protocol  # the shell, serving ui/dist (run `pnpm build` first)
OKB_WEBVIEW_ISOLATION=1 cargo test -p openkoalabot-example-tauri --features custom-protocol --test webview_isolation  # macOS: a temp-HOME run writes nothing under the real ~/Library
```

Webview data stays in the app's data directory (`$HOME/Library/Application Support/ai.cua.openkoalabot.example` on macOS): a data directory under it on Windows and Linux, a non-persistent store on macOS. The page's saved state (Bots, selection, threads) is `ui-state.json` there, not web storage.

The hermetic tests run against the SDK's in-process mock cua-spacesd
(`cua_spacesd_client::testing`) on loopback: processes are simulated and
files live in memory, so nothing runs on or writes to the host. The mock has
no file transfers, so there a drop is checked to fail loudly; the docker lane
of the scenario covers real drops. Teleport is checked to refuse with the
Cua Spaces message. `tests/plan.rs`
checks that a wizard plan reaches the SDK as the right call (a recording fake
`LocalRuntime`, no container or VM is started).

## Scenario

```sh
cd src-tauri && cargo build -p openkoalabot-example-core --bins
../../openkoalabot-example-scenario/run.sh --impl tauri --lane fixture   # no docker
../../openkoalabot-example-scenario/run.sh --impl tauri --lane docker    # linux, runsc, 4 GiB
```

The same behaviour ships in [`openkoalabot-example-swift`](../openkoalabot-example-swift)
and [`openkoalabot-example-ts`](../openkoalabot-example-ts); all three pass the shared
scenario in [`openkoalabot-example-scenario`](../openkoalabot-example-scenario).

## License

Source-available under [FSL-1.1-MIT](LICENSE): its live stream uses the
streaming client that ships with Cua Spaces (`cua-spaces-ext`). Each release
becomes MIT two years after it ships.
