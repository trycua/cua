# Bridge

The bridge connects the web UI to whichever host runs it. It loads the
Cua Spaces app core as wasm, so behaviour stays in Rust. Screens import
only from `src/bridge` (`index.ts`).

```
 screens ──► hooks (useSpaces, useMachines, useSettings, useKeyvault, useSession,
               │        useAgents, useAgentTimeline)
               │
           BridgeStore ──► app core (wasm): rowsToSpaces, creates.reduce/compose,
               │            settings.page, keyvault.page/sidebar/vaultView/vaultReduce,
               │            wizard.validateImageRef, agents.pageView/name/order
               ▼
          DataAdapter  (protocol.ts: one operation table, one event union)
   ┌──────────┬──────────────────────────────────┬─────────────┐
   tauri      webkit (native hosts)              demo
   invoke()   postMessage (SwiftUI) or           in memory
              cuaDesktop (Electron), transport.ts
```

## Contract for the UI

```tsx
import { BridgeProvider, useSpaces } from "@/bridge";

<BridgeProvider>{/* app */}</BridgeProvider>
```

| Export | Returns |
|---|---|
| `useBridge()` | `{ mode: "tauri" \| "electron" \| "webkit" \| "demo", core, data }`. `core.status` is `loading`, `ready` or `unavailable`; `data` is the `DataAdapter`. |
| `useSpaces()` | `Space[]` with pending creates, power changes and deletes overlaid by the core. Actions: `createSpace(req)`, `cancelCreate(pendingId)`, `dismissCreate(pendingId)`, `startSpace(id)`, `stopSpace(id)`, `deleteSpace(id)`, `openSpace(id)`. |
| `useMachines()` | `Machine[]`, this machine first, each with `spaceIds`, `connection` (`relay`, `direct`, or null before host setup), `detail`, `lastSeen` and, for this machine, `panel` (the core's `host.panel`). Also `setupGuide`: host setup in the core's words (`host.panel` choices, `host.formView`), for the Add a machine explainer. |
| `useSettings()` | `{ values, telemetry, defaultLocation, page }`, where `page` is the core's Settings layout. Action: `updateSetting(key, value)` for `theme`, `menuBar`, `hotkey`, `telemetry`, `defaultLocation`, `launchAtLogin` or `updateChannel`. The last two are `null` when the host can't offer them, and the Settings page hides their rows. |
| `useKeyvault()` | `{ overview, groups, views, vaultState }`. `views` holds the core's `page`, `sidebar` and `vault` (the list grouped by app, then site). Actions: `unlock(passphrase?)`, `lockItem(id)`, `lockItems(ids)`, `unlockItems(ids)`, `setDisabled`, `approve`, `deny`, `revokeGrant`, `vaultAction(action)` (search, select, open), and `pane(selection)`. |
| `useSession()` | `{ fleet, onboarding, daemon, signedIn, identity, signIn }`, where `signIn` is the sign-in phase. Actions: `signIn()`, `signOut()`, `completeOnboarding(mode)`, `openExternal(url)`. |
| `useAgents()` | `{ agents, runs, unread, canList }`: persistent agents (state `running`, `paused` or `idle`, the core's row line and action label, the run the detail opens) and the runs in every running Space, in the core's order. Actions: `pauseAgent(name)`, `resumeAgent(name)`, `agentSetup()`, `configureAgents(agents?)`. |
| `useAgentTimeline(run)` | One run's conversation while mounted: `{ items, status, phase, caughtUp, isLoading, error, unsupported, lastEventMs }`. It reads `agents.events` with a cursor (every 500 ms while a turn runs, 4 s otherwise) and folds the events like cua-agents' `Transcript` (`transcript.ts`). Items are immutable, so a row memoized by item re-renders only when its item changed. |
| `useOnboarding()` | The first run from the core (`onboarding.initial/reduce/view/copy`): `{ state, view, copy, signInText, permissions, hostConfigured, images }`. Actions: `send(action)`, `skip()`, `finish()`, `restart()`. See below. |
| `useSavedOnboarding()` | `{ state, skipped }`: the saved progress the shell reads to reopen or offer the first run. |
| `useVolume()`, `useExperimentFlags()`, `useOnboardingVolume(flow)`, `useDriverSetup()` | Cua Volume (`volume.ts`): the Volume page `{ view, unsupported, act(action) }`, Settings, Experiments, the first run's Cua Volume commands, and the cua-driver setup. See "Cua Volume (W4)". |

Every hook returns `{ data, isLoading, error, refresh }` plus its actions.
Actions return promises and refetch what they change. Host events (registry
changed, create progress, sign-in finished) refetch on their own.

**Lock and unlock.** In the Keyvault, *locked* means an item asks for each
use and *unlocked* allows unattended access (`policy.unattended`). Both map
to the broker's existing `SetUnattended`. The core documents it as
`SetLocked` with `locked` reversed. Unlocking asks for Touch ID natively.
`unlock()` with no argument unlocks the vault itself, with the OS protector.

**Types** (`contracts/`) are hand-written mirrors of the Rust view models,
each file naming its source in `libs/cua/crates/cua-spaces-app-core`. The
core has no ts-rs or specta derive. Once the core exports its types,
generate these and delete the mirrors. The Tauri app's `src/native/keyvault.ts` is out of
date (it predates one item per secret). `contracts/keyvault.ts` follows
`wire.rs` instead.

## The core

`core/index.ts` loads `core/wasm/core.js`, built by
`scripts/build-core-wasm.mjs` from the Tauri app's shim
(`apps/cua-spaces/core-wasm`). That is the same crate and the same
`call(method, argsJson)` export, with nothing new on the Rust side. It is
plain wasm, so it loads the same way in every host. If it wasn't built or
fails to load, `core.status` is `unavailable` and `derive.ts` falls back to
small TypeScript stand-ins. These cover what the demo needs. They are not the
product's behaviour, and parity runs only against the core path.

## Hosts

Detection order: `__TAURI__` (or `__TAURI_INTERNALS__`), then
`window.cuaDesktop`, then `window.webkit.messageHandlers.cua`, then demo.
`?bridge=demo` forces demo inside a host. `?demo=fresh` starts signed out at
onboarding, and `?demo=locked` starts with the Keyvault locked; the two can be
combined (`?demo=fresh,locked`).

Every host answers the same operations (`protocol.ts` → `HostOperations`).
Each is an existing Tauri command under a host-neutral name:

| Operation | Tauri command |
|---|---|
| `spaces.list` | `list_spaces` |
| `spaces.create` `{config, pendingId}` | `create_space` |
| `spaces.cancelCreate` `{pendingId}` | `cancel_create` |
| `spaces.setPower` `{spaceId, on}` | `set_space_power` |
| `spaces.delete` `{spaceId}` | `delete_space` |
| `spaces.open` `{spaceId, name?, os?}` | `open_space_window` (`space: {id, name, os}`) |
| `machines.list` | `host_status` + `list_hosts` + `get_environment` |
| `host.status` | `host_status` |
| `settings.get` / `settings.set` `{key, value}` | UI storage (`ui_storage_set`), `telemetry_status`/`telemetry_set_enabled`, `get_default_location`/`set_default_location`, `login_item_status`/`login_item_set` (`launchAtLogin`). The Tauri updater has no channels, so `updateChannel` is `null` there. |
| `settings.choose` `{row, option}` | none: the Tauri shell lays out none of the host's own rows (`SettingsSnapshot.hostSettings`: the SwiftUI app's Runtimes, auto-connect and Keyvault auto-wipe, which the page draws from the core's Settings page and changes with `settings.choose`; New Space's "Use built-in Lume" too). |
| `keyvault.overview` | `keyvault_overview` |
| `keyvault.unlock` `{passphrase?}` | `keyvault_unlock` / `keyvault_unlock_passphrase` |
| `keyvault.setUnattended` `{itemIds, unattended}` | `keyvault_set_unattended` |
| `keyvault.setDisabled`, `.approve`, `.deny`, `.revokeGrant` | `keyvault_set_disabled`, `keyvault_approve`, `keyvault_deny`, `keyvault_revoke_grant` |
| `session.get` | `fleet_status` + `onboarding_state` + `daemon_status` |
| `session.signIn` / `.signOut` / `.completeOnboarding` / `.openExternal` | `begin_sign_in` / `sign_out` / `complete_onboarding` / `open_external` |
| `agents.list` | `agents_tool` `persistent_agent_list` (snake_case mapped to `PersistentAgent`) |
| `agents.runs` `{spaceId}` | `list_space_agents` |
| `agents.events` `{spaceId, runId, cursor, max?}` | none yet: unsupported (`TAURI_UNSUPPORTED`) |
| `agents.pause` / `.resume` `{name}` | `agents_tool` `agent_pause` / `agent_resume` |
| `agents.setup` / `.configure` `{agents}` | `agent_setup_detect` / `agent_setup_configure` |

Events (`HostEvent`): `spaces.changed`, `spaces.createProgress`,
`machines.changed`, `settings.changed`, `keyvault.changed`,
`session.signedIn`, `session.signInFailed`, `session.signedOut` and
`agents.changed` (no Tauri event yet; the store also refetches agents when a
watched run starts or ends, and when Spaces change). They
correspond to the Tauri events `spaces:changed`, `spaces:create-progress`,
`auth:signed-in`, `auth:sign-in-failed` and `auth:signed-out`. Without
`app.withGlobalTauri` there is no event API, and the bridge polls
`spaces.list` every 10 s.

### Electron: `electron-channels.ts`, `transport.ts`

The Electron shell is a native host like the SwiftUI app: its main process
loads the same Rust library and answers the methods below
(`webkit-protocol.ts`) with the same shapes, error codes and events
(apps/cua-spaces-desktop, `src/bridge/`). The page runs the webkit adapter
over the preload's `window.cuaDesktop` (`createElectronAdapter`): every
request is `invoke("cua:bridge", { id, method, args })` and resolves to the
same envelope, and the host's events arrive on `cua:event` as
`{ event, payload }`. The preload refuses every other channel. Only
Electron answers `ELECTRON_HOST_METHODS`: `spaces.openStream` (it draws a
Space's video in the page) and `onboarding.get` / `onboarding.complete`
(its first run is this page's `routes/onboarding`, with Done's "Launch at
login"; the SwiftUI app's is native, so there `session.get` reads it as
done). Electron reads drag regions from the CSS, so the adapter sends none
there.

### SwiftUI WKWebView: `webkit-protocol.ts`

The macOS app's `WebUIBridge.swift` defines this contract, and the adapter
speaks it as is:

- The page sends `{ id, method, args }` through `window.webkit.messageHandlers.cua.postMessage`.
  That is a `WKScriptMessageHandlerWithReply`, so it resolves with
  `{ id, ok: true, result }` or `{ id, ok: false, error: { code, message } }`.
  Requests time out after 30 s.
- The methods are the host's own (`WEBKIT_METHODS`): `spaces.list`,
  `spaces.setPower {id, on}`, `spaces.open {id}`, `settings.choose {row, option}`,
  `keyvault.unlock {ids}` and so on. Results are the app core's view models
  (camelCase records). `adapters/webkit.ts` maps each operation onto them and
  turns the results back into the wire shapes above, so the store derives
  the same way as for every other host. `AppSpace` becomes `SpaceRow`, the
  Settings page rows (`notch`, `default-location`, `telemetry`) become
  `SettingsValues`, and the Keyvault overview becomes snake_case.
- Events arrive as `window` `cua:event` CustomEvents (`detail: { event }`):
  `spaces.changed` (it also refreshes machines), `machines.changed` (this
  Mac's host status or the account's devices), `keyvault.changed`,
  `settings.changed`, `session.changed` and `agents.changed`.
- `spaces.cancelCreate {pendingId}`, `host.status`, `keyvault.approve
  {requestId, items}`, `keyvault.deny {requestId}`, `keyvault.revokeGrant
  {id}` and the seven `agents.*` methods keep the operation's own name and
  arguments. They go to `AppModel.cancelCreate`,
  `HostModel.state` and the Keyvault broker commands the native approval
  sheet sends. Waiting Keyvault requests (`pending`) come back tagged as the
  broker tags them.
- New Space is the page's wizard: `spaces.createOptions` answers the env the
  native sheet opens with (`AppModel.newSpaceEnv`: this Mac's runtimes,
  storage and GPUs, your machines that provide Spaces, your clouds), carried
  as `NewSpaceOptions.env` (with `local`, `gpus` and `cloudPricing` from it,
  and `macosVmsRunning`: the macOS VMs Lume runs on this Mac, which Apple's
  limit of two counts). While the launch is still starting it answers at
  once with what it knows and `pending: true`; the page asks again on
  `startup.changed` (ready). `spaces.create {config, pendingId, os}` runs
  the sheet's create (`AppModel.runCreate`; the notch shows it too), with
  `spaces.createProgress` events until it answers with the Space (or
  `cancelled`). The app's own New Space items (⌘N, the menu bar, a machine's
  "New Space on…") send `spaces.newRequested {on}`, which opens the wizard;
  the native sheet is the fallback while New UI is off. The app menu's
  Settings command (⌘,) sends `settings.openRequested` while the New UI
  window is in front, and the page goes to Settings (also with no focus in
  its web view).
- `machines.list` carries `hostnames` (what each relay machine's cua-spacesd
  reported), so a machine and its enrolled device list once.
- `keyvault.unlock` with a passphrase fails with `native_only`.
  Onboarding stays native. Appearance and the hotkey are kept in the page,
  and an appearance change calls `window.setBackgroundColor`.
- Touch ID and the unlock prompt stay native.
- Launch at login is the core's `launch-at-login` row. The adapter reads it
  from the page and sets it with `settings.choose {row: "launch-at-login"}`,
  which `AppModel.choose` already handles. While the row is missing or
  disabled (login item not read yet, or not found) the value is `null`.
- The update channel: `settings.get` sends `updateChannel` (`stable` or
  `beta`, `UpdatesModel.channel`; null when the build has no updater, and
  the row stays hidden), and `settings.choose {row: "update-channel",
  option}` goes to `model.updates.choose(channel:)`.
- `machines.list` carries `host` (the same record as `host.status`), which
  the adapter maps to `HostStatus` on this Mac's row, so the Machines page
  shows the core's "This machine" panel.

Teleport an app and Share (`teleport.*`, `sharing.*`): operations, Tauri commands and the Electron and Swift host notes are in `ops/teleport.ts` and `ops/share.ts`.

`coverage.ts` lists, for every operation, the WebKit methods, the Electron
channel and the Tauri commands it uses. `__tests__/coverage.test.ts` checks
the list against each adapter and against `WebUIBridge.methods`, and fails
when WebKit misses an operation without a `byDesign` note.
It also fails when the Swift host routes a method no operation calls
without a reason in `WEBKIT_HOST_ONLY`, and when an `unsupported` or
`byDesign` note is stale.

### The bridge contract (`contracts/shapes.ts`)

Every operation's result has one shape (`OP_SHAPES`), and every SwiftUI
method's answer has one too (`WEBKIT_SHAPES`, the shapes `adapters/webkit.ts`
reads). The builders are checked against the contract types, so a field
added to a contract doesn't compile until its shape names it.
`contracts/bridge-shapes.json` is the export (JSON Schema keywords); after a
change, run `pnpm contract:shapes`. Checked against:

- the demo host on each platform it plays (`__tests__/contract.test.ts`);
- the Electron host on the real app core over fixtures, through the
  Electron adapter (`apps/cua-spaces-desktop/test/bridge-native.test.ts`),
  and the built shell's preload (`e2e/electron-bridge.spec.ts`);
- `WebUIBridge` on fixture backends, which also checks every listed method
  is routed and every routed one listed (`BridgeContractTests.swift`); its
  answers then go through the webkit adapter (`CUA_BRIDGE_ANSWERS`).

`pnpm contract` runs all of it locally (`-- --electron` adds the shell and
its parity flows; `-- --no-swift` skips the Swift part).

Tauri and Electron use the same JSON shapes (`contracts/`); WebKit maps onto them. Nothing here is
a new protocol or security mechanism. It is a typed transport for the
existing commands. Passphrases pass straight through to the broker, as they
do in the Tauri app.

### Machines: optional fields (all hosts)

`machines.list` rows (`MachineRow`) may carry, all optional and additive:

- `detail`: one line in the host's words (this machine's `host_summary`, a device's `DeviceRow.detail`).
- `lastSeen`: Unix seconds (`DeviceRow.lastSeen`).
- `host`: on the `current` row only, the `HostStatus` from `host_status`. The page draws it with the core's `host.panel`.
- A `limits` entry with `resource: "sharing"` (`used` 0, `limit` 0, `reason` the line to show): the machine is online (the relay sees it connected) but its owner stopped sharing it, so it refuses every call. It is not a limit: `Machine.notSharing` carries it to the page ("Online, not sharing") and the core's wizard reads it for New Space's Run on ("(not sharing)", `wizard::HOST_SHARING_STOPPED`), so both say the same thing. The SDK's `Spaces.hosts()` (the SwiftUI host's wizard env) reports it so; the native hosts' Machines rows set it from the list's connect probe failing with "stopped sharing".

Tauri fills `host` from the `host_status` it already calls. **Native hosts** (`WebUIBridge`, and the Electron shell's port of it): `machines.list` sends `thisMachine`, `devices` (read for `detail` and `lastSeen`) and `host` (`HostModel.state`, mapped to `HostStatus`). Relay machines that share their desktop (`relay:<id>` in `spaces.list`) are listed with their online state. Host setup and the This machine buttons are `host.setUp` and `host.action` (below).

## Agents

The Agents page reads persistent agents, the runs in each running Space, and
one run's events. Every operation is an existing SDK tool or shell command
under a host-neutral name; nothing here is a new mechanism. Types are in
`contracts/agents.ts`, each naming its Rust source.

| Operation | Args | Result | Source |
|---|---|---|---|
| `agents.list` | `{}` | `PersistentAgent[]` | `persistent_agent_list` |
| `agents.runs` | `{ spaceId }` | `SpaceAgentRun[]` | `agent_list` (`list_space_agents`) |
| `agents.events` | `{ spaceId, runId, cursor, max? }` | `AgentEventsPage` (`{ run_id, status, phase, events, cursor, caught_up }`) | `agent_events` |
| `agents.pause` / `agents.resume` | `{ name }` | `null` | `agent_pause` / `agent_resume` |
| `agents.setup` | `{}` | `AgentSetupRow[]` | `agent_setup_detect` (`cua agents status`) |
| `agents.configure` | `{ agents: string[] \| null }` | `AgentSetupRow[]` | `agent_setup_configure` (`cua agents setup`) |

Event: `{ type: "agents.changed" }` when an agent is paused, resumed or saved,
or a run starts or ends. The store refetches `agents.list` and the runs.

A host that can't answer an operation rejects with `UnsupportedOperationError`
or a `HostError` with code `unsupported` (`isUnsupported(e)`). The page then
says what is missing instead of showing an empty list: no list at all, or a
run whose conversation can't be read.

| Host | Answers |
|---|---|
| demo | everything: four persistent agents (Claude Code, Codex, Hermes, OpenClaw) and five runs; `run-ada-7` writes its third turn live for about 25 s after start |
| tauri | everything except `agents.events` (the Tauri shell has no `agent_events` command, and `agents_tool` doesn't allow it) |
| electron | the Swift host's methods (below), as they are ported (`apps/cua-spaces-desktop/test/not-yet.ts`) |
| webkit | everything, from the SwiftUI app's own models (below); without the daemon the host answers `unsupported` |

### For the Swift host (`WebUIBridge.swift`)

The host routes these methods. The names and args match the operations, and
the results are the same JSON (camelCase records except `agents.events`,
which is the `agent_events` result as is):

| Method | Args | Result | SwiftUI source |
|---|---|---|---|
| `agents.list` | `{}` | `PersistentAgentInput[]` | `PersistentModel.loadAgents` (`persistent_agent_list`) |
| `agents.runs` | `{ spaceId }` | `SpaceAgentRun[]` | `AgentRunsModel.refresh` |
| `agents.events` | `{ spaceId, runId, cursor, max? }` | `agent_events` result | the `agent_events` tool (`AgentRun.events(cursor:)` in the SDK) |
| `agents.pause` / `agents.resume` | `{ name }` | `null` | `PersistentModel.send(.pause/.resume)` |
| `agents.setup` / `agents.configure` | `{}` / `{ agents }` | `AgentSettingsRow[]` | the Settings "AI agents" rows |

The host sends `agents.changed` when the persistent agents or the AI agents
rows change. Without the daemon (`persistent_agent_list`, `agent_events`) or
the SDK's agent setup, a method fails with code `unsupported`, and the page
says what is missing.

`__tests__/agents.test.tsx` covers the transcript fold, the demo's live run
and pause, and `useAgents` / `useAgentTimeline` streaming a run to its end.

## New Space (`new-space.ts`, `ops/new-space.ts`)

The wizard and "Connect a cloud" run the core's `wizard.*` and `cloudConnect.*`; the five host operations they add, and what the native hosts need for them, are in `ops/new-space.ts`.

## First run (`onboarding.ts`)

The pages, copy and answers are the core's, the same as `OnboardingView.swift`.
One core page needs the native app, so the web passes through it with the
answer an unchanged native run gives: Menu bar (`presentation-picked` with
the current `menuBar` setting). Left: Welcome with the usage-data switch,
Sign in (optional), AI agents (detected agents, the cua skills and MCP
server, and the cua-driver card; see "Cua Volume" below), Cua Volume (only
with its experiment on), This machine with the permissions still to grant
(`host.permissionRows` over `host.status`'s hints; explainers only, the
grant stays in System Settings), and Done with a first Space
(`wizard.pickerImages`, one per OS, created through
`useSpaces().createSpace`).

- Progress is saved in `localStorage` (`cua-spaces:onboarding`), so the flow
  resumes on the same page. "Set up later" sets `skipped`; the shell stops
  opening it and the sidebar offers "Finish setting up".
- The usage switch writes `settings.set {telemetry}` at once, and shows only
  once the host has reported the setting (`settings.get`'s `telemetry`).
- `finish()` calls `session.completeOnboarding {mode}` and forgets progress.
- No new operations: hosts answer `host.status` (permissions, `configured`),
  `settings.set`, `session.*`, as before.
  Swift host (`WebBridge`): onboarding stays native, and the shell never opens
  the web flow in `webkit` mode.

## Parity flows

The 33 goldens in `cua-spaces-app-core/parity/*.json` run through the
core-wasm exports `flows()` and `runFlow(name, flow, host)`. In those calls,
`host` is a `(method, argsJson) → resultJson` function.

- **Core path:** `core.parity.flows()` and `core.parity.run(name, flow, host)`. `__tests__/core.test.ts` replays every golden with `host = core.call`.
- **Bridge and screens:** `pnpm parity` (Playwright, `e2e/`). It builds the wasm core, serves the app and opens each screen with `?bridge=demo&parity`. `parity.ts` installs `window.__cuaParity` only in that case. `replay(name)` answers every core call the bridge makes for a screen (`spaces.rowsToSpaces`, `creates.*`, `keyvault.*`) with the bridge's own function and sends the rest to the core. The transcript must equal the golden. Each state the replay reaches then goes on the screen through the store (`showSpaces`, `showKeyvault`), and the test checks what the screen shows.
- `e2e/flows.ts` lists every flow as run (with its screen) or skipped (with the screen it waits for). A flow on disk that is missing there fails the run. When a screen lands, switch its flows to run and add its checks to `parity.spec.ts`.
- `pnpm parity:electron` runs the same specs in the built Electron shell (demo mode, no cua, its window hidden; `e2e/host.ts`), plus `e2e/electron-bridge.spec.ts`.
- The run prints a table (flow, status, reason) and writes it to `test-results/parity-summary.md`. The goldens are only read. To change one, change the core and run `UPDATE_PARITY=1 cargo test -p cua-spaces-app-core --test parity`.

## Cua Volume

The Volume page, the first run's Cua Volume page and the cua-driver card.
The page and the first-run page show only with Settings, Experiments'
**Cua Volume** on (`experiments.get`, owned by Settings in `ops/settings.ts`;
`useExperimentFlags()` reads it), as in the native app. Decisions are
the core's: `drive.initial/reduce/view` for the page, `onboarding.*` with the
drive actions and `storage.edit/choose` for the first-run page,
`onboarding.drivePreview*` and `onboarding.driverPreview*` for the two
miniatures, `agents.setupSummary` for the lines after a setup. `volume.ts`
runs the command the core asks for, like `PersistentModel.sendDrive` and
`OnboardingModel` do. Operations are in `ops/volume.ts`, demo data in
`adapters/demo/volume.ts`; each is a daemon Spaces tool or shell step the
native app already runs.

| Operation | Args | Result | Source |
|---|---|---|---|
| `volume.overview` | `{}` | `VolumeOverview` (`{os, home, requests, grants, mount, sync}`; `mount`/`sync` null when unanswered) | `volume_requests`, `volume_grants`, `volume_mount_status`, `volume_sync_status` |
| `volume.storage` | `{}` | `volume_storage` or null | `volume_storage` |
| `volume.storageSet` | `{ update }` (the core's `DriveStorageUpdate`, snake_case) | `volume_storage_set`'s check | `volume_storage_set` |
| `volume.mount` / `volume.unmount` | `{}` | the mount status after it | `volume_mount` / `volume_unmount` |
| `volume.approve` / `volume.deny` | `{ id }` | `null` | `volume_approve` / `volume_deny` `{request_id}` |
| `volume.revoke` | `{ id }` | `null` | `volume_revoke` `{grant_id}` |
| `volume.resolve` | `{ path }` | `null` | `volume_sync_resolve` |
| `volume.reveal` | `{ path }` | `null` | show a path inside the mounted volume in Finder (Tauri `drive_reveal`) |
| `agents.setupDriver` | `{ agents }` | `AgentSetupOutcome[]` | `cua agents setup --cua-driver` (`AgentSetupRunning.setUpCuaDriver`) |

| Host | Answers |
|---|---|
| demo | everything; the experiment is on, one request, one grant, two devices with a conflict, mounted (off with `?demo=fresh`) |
| tauri | everything through `agents_tool` and `drive_reveal`, except `agents.setupDriver` (no cua-driver command; `TAURI_UNSUPPORTED`) |
| electron | the Swift host's methods, as they are ported |
| webkit | everything, under the same names (`WebUIBridge+Pages.swift`); without the daemon each rejects as `unsupported` |

**Swift host (`WebUIBridge+Pages.swift`).** The same names and args; the
results are the daemon's answers as `PersistentModel` and `OnboardingModel`
read them: `volume.*` through `AgentsToolRunning.agentsTool` (`volume.reveal` is
`PersistentModel.reveal`, for a path in the home folder only), and
`agents.setupDriver` through `AgentSetupRunning.setUpCuaDriver(agents:)`.
`settings.changed` follows the settings, experiments included.

Parity: `drive-page` (the Volume page), `drive-onboarding` (the first run)
and `driver-card` (Settings, AI agents) replay through `parity-volume.ts`;
`e2e/volume.ts` puts each state on its screen. `PARITY_PORT` serves the app
on another port when several worktrees run parity at once.
## Settings and Notifications (`ops/settings.ts`, `ops/notifications.ts`)

Settings has the native window's tabs: General (launch at login as the
core's `login_item::rows_with`, and Storage after General while the Cua
Volume experiment is on), Devices, Experiments and About. Notifications is
the daemon's feed. `settings-feature.ts` holds their state and derives
through the core (`settings-derive.ts`); hooks are `useAbout`,
`useDevices`, `useStorageSettings`, `useExperiments`, `useLoginItem` and
`useNotifications`. Each is an existing host call; nothing is a new
mechanism. Types are in `contracts/settings.ts`, `contracts/devices.ts` and
`contracts/notifications.ts`.

| Operation | Args | Result | Tauri | SwiftUI source |
|---|---|---|---|---|
| `about.get` | `{}` | `AboutInput` | `plugin:app\|version`, `get_environment` (no updater) | `UpdatesModel.input` |
| `about.set` | `{ autoCheck?, autoInstall?, channel? }` | `AboutInput` | unsupported | `UpdatesModel.setAutoCheck/setAutoInstall/choose(channel:)` |
| `about.checkNow` | `{}` | `AboutInput` | unsupported | `UpdatesModel.checkNow` |
| `experiments.get` / `.set` | `{}` / `{ experiments }` | `Experiments` | UI storage `cua.settings.experiments` | `app.info`, `settings.choose {row: "experiment:…"}` (routed today) |
| `loginItem.get` / `.set` | `{}` / `{ on }` | `{ status, providesSpaces, runsAgents }` | `login_item_status` / `login_item_set`, `host_status`, `agents_tool` | `LoginItem.status()`, `register/unregister`, `HostModel.provideSpaces`, `PersistentModel` |
| `loginItem.openSettings` | `{}` | `null` | `host_open_settings` | `SMAppService.openSystemSettingsLoginItems()` |
| `devices.get` | `{}` | `DevicesInput` | `devices_snapshot` | `DevicesModel` snapshot (`devices.snapshot()`), as camelCase `DevicesInput` |
| `devices.enroll` / `.checkEnrolled` | `{}` | `{ enrolled, code }` / `boolean` | `devices_enroll` / `devices_check_enrolled` | `devices.enroll()` / `checkEnrolled()` |
| `devices.approve` | `{ code, deviceId }` | `null` | `devices_approve` (presence in the shell) | `presence.confirm` then `devices.approve` |
| `devices.rename` / `.revoke` / `.confirmMachine` | `{ id, name? }` | `null` | `devices_rename` / `devices_revoke` / `devices_confirm_machine` | same calls on `DevicesRunning` |
| `storage.get` | `{}` | `StorageInput` (`volume_storage`, `volume_mount_status`, `volume_cache_stats` as the tools answer) | `agents_tool` | `StorageModel.load` inputs |
| `storage.run` | `{ request: StorageRequest }` | `DriveCheckInput` for test, save and adopt; else `null` | `agents_tool` (`volume_*`), `drive_reveal`, `host_open_settings` | `StorageModel`'s request runner |
| `notifications.list` | `{}` | `NotificationInput[]` (camelCase) | `agents_tool` `notifications_list` | `PersistentModel.feed` |
| `notifications.markAllRead` | `{}` | `null` | `agents_tool` `notifications_ack {ids: []}` | `PersistentModel.markAllRead` |

**Electron.** The Swift host's methods, as they are ported
(`apps/cua-spaces-desktop/test/not-yet.ts`).

**Swift host (`WebUIBridge+Pages.swift`).** Experiments work over
`app.info` and `settings.choose`; every other operation here is a method of
its own name with the args above (`ops/webkit-pages.ts`). Without an
updater (`about.set`, `about.checkNow`), a login item, a signed-in account
(`devices.*`) or the daemon (`storage.*`, `notifications.*`) the host
answers `unsupported`.
Notifications post as toasts only outside the SwiftUI host, which posts its
own system notifications. Send `agents.changed` when the feed changes.

## Space detail, This machine and usage events (round 3, W5)

New operations live in `ops/<feature>.ts` (types, host mappings, coverage
rows) and the demo's answers in `adapters/demo/<feature>.ts`. Each is
something the native apps already do; nothing is a new mechanism.

| Operation | Args | Result | Tauri | SwiftUI (`WebUIBridge+Pages.swift`) |
|---|---|---|---|---|
| `telemetry.track` | `{ signals }` | `null` | `telemetry_record_signals` | `model.telemetrySink?.record(signals)` (`appTelemetryRecord`), only while `status().enabled`; each signal checked again |
| `spaces.usage` | `{ spaceId }` | `SpaceUsage \| null` | `space_usage` | `backend.usage(id:)` |
| `spaces.windows` | `{ spaceId }` | `{ windows, display }` | none (the viewer lists windows over its own stream) | `StreamRowsModel` over `backend.streamProvider(id:)` |
| `stream.pip` | `{ spaceId, command: { type: "open" \| "close", row } }` | open row ids | none (its PiP is a viewer window) | a `StreamPiPSet` per Space, as the detail view keeps |
| `spaces.thumbnail` | `{ spaceId, maxAgeMs? }` | `{ url, capturedAtMs } \| null` | none | `SpaceThumbnails.refresh` (the notch's and the cover's image), as a JPEG `data:` URL (`WebUIBridge+Thumbnails.swift`) |
| `host.setUp` | `{ request }` (`host.formView`'s `request`) | `HostStatus` | `host_setup` | `HostModel.setUp` (the form's submit); no timeout (it may wait for the sign-in in the browser) |
| `host.action` | `{ action }` (`stop-sharing`, `resume-sharing`, `remove`, a switch, `sign-in`) | `HostStatus` | `host_stop_sharing`, `host_start_sharing`, `host_remove` + `host_status`, `host_configure` | `HostModel.run(action)`; no timeout |
| `host.openSettings` | `{ url }` (`x-apple.systempreferences:` only) | `null` | `host_open_settings` | `PermissionRows.open` |

WebKit routes all seven under these names (`ops/webkit-pages.ts`). A failed
`host.setUp` or `host.action` is worded as the native page words it
(`HostSetupFailure`): the error's `message` is what to do, with `title`,
`details` (the raw error) and `actionLabel` (Retry, or Sign In) beside it;
the page shows them with Retry, which sends the same request or button
again. `host.status` also carries `progress` (`HostModel.progress`: what a
running setup or Sign In waits for).
`machines.list`'s current row may carry `arch` (`aarch64`, `x86_64`) for the
facts' emulation warning; the Swift host can send `appHostArch()`.

**Usage events.** The store asks the core what each step means
(`telemetry.launched`, `.onboarding`, `.onboardingFinished`, `.creates`;
`telemetryStorage`, `telemetryShare` and `telemetryEnroll` are ready for the
screens that drive those reducers) and `TelemetryForwarder` sends them. It
sends only while `settings.get`'s `telemetry` says on and the notice was
shown, read before every batch; with the first-run switch off the core
derives nothing. `sanitizeSignals` drops any signal that isn't a known type
with exactly its fields as fixed lowercase words, flags or durations, so
names, emails, paths and URLs can't leave the page. Tauri and the SwiftUI
app record `app_launched` themselves (`HOST_RECORDS_LAUNCH`); the page
records it in Electron and demo.

**Space detail.** `useSpaceDetail(space, hostArch)` gives the core's
`sidebar.detail` (facts, sections, Delete's question) and
`sidebar.streamSection` (rows, picture-in-picture buttons), and reads usage
every 10 s and windows every 5 s while mounted. `<StreamSurface spaceId>`
(`components/space-detail/stream-surface.tsx`) is where live video goes;
its layout contract is in that file. It is a placeholder with Open window,
except in the SwiftUI host (native video, on unless `WebUINativeVideo` is
set to NO) and the Electron shell (WebCodecs): there
it reports its rect, visible part and whether page UI covers it on the
`cuaVideo` handler, and the host draws the Space over it and takes its input
(`src/lib/stream-surface.ts`, `src/lib/video-slots.ts`,
`WebUIVideoSurfaces.swift`; apps/cua-spaces-macos/docs/native-video.md). Space
tiles on the grid do the same at a low rate (`components/video/tile-video.ts`).
A PiP press on a host without panels opens the Space's window.

**This machine.** `useMachines()`'s current machine `panel` is the page;
`useThisMachine(identity)` is the setup form (`host.formReduce/formView`)
and the buttons.

**Parity.** `e2e/flows.ts` marks `notch`, `notch-drag-trigger` and
`menu-count` `native`: the Swift parity runner covers them, and the summary
counts them apart from skips. Set `PARITY_PORT` when several worktrees run
`pnpm parity` at once, so each gets its own server.

## Startup screen (`ops/startup.ts`)

While the native app is still starting (reading the saved sign-in from the macOS Keychain, or starting the daemon), the root layout shows `components/startup/StartupScreen.tsx` instead of the app; `useStartup()` reads it. The host owns the state and the words.

| Operation / event | Args / payload | Result | Tauri | SwiftUI |
|---|---|---|---|---|
| `startup.get` / `startup.act` | `{}` / `{ action: "allowAccess" \| "tryAgain" \| "signInAgain" }` | `StartupState` (`phase`, `slow`, `title`, `body`, `actions`) | none: ready (no startup gate) | `startup.get` / `startup.act`; `unimplemented` or any failure reads as ready |
| `startup.changed` (event) | `StartupState` | | | `cua:event` `{ event: "startup.changed", payload }` |

Electron answers the same methods: its launch is the SwiftUI app's (the keychain check on macOS, the daemon, the SDK). The demo is ready unless `?demo=keychain`.
