# OpenKoalaBots (TypeScript)

A web app where you hire named Bots, give each one its own computer (a
Space), and watch it work beside the thread. A small Node server on
[`@trycua/cua`](../../libs/cua/typescript) runs the agents; the page is plain
TypeScript built with Vite.

```sh
npx degit trycua/cua/samples/openkoalabot-example-ts my-app
cd my-app && npm install
npm run build && npm run build:web && npm run serve
```

> `@trycua/cua` is not published on npm yet, so a degit copy does not
> install on its own today. Until it is, develop inside the cua repo (below).

## Developing inside the cua repo

The SDK dependency is a local workspace override:

| Dependency | Where it comes from today |
|---|---|
| `@trycua/cua` | `file:../../libs/cua/typescript` in `package.json` (native Node library plus the wasm browser build) |
| The sandbox image list | `../../libs/images/sandbox-images.json`, imported by the page (`web/main.ts`) and read by the server (`src/core/plan.ts`) |

Once `@trycua/cua` is on npm, the `file:` dependency becomes a version and
the quick start above works as is.

```sh
# 1. The SDK: native library for Node, and the wasm browser build
cd libs/cua && cargo build --release -p cua-sdk
cd typescript && npm ci && node ../scripts/stage-uniffi-library.mjs --only=node && npm run build && npm run build:browser

# 2. This example
cd ../../../samples/openkoalabot-example-ts
npm ci
npm run build && npm run build:web

# 3. The server; open the URL it prints (it carries #token=…)
npm run serve                       # embedded runtime, state in ~/.openkoalabot-example-ts
npm run serve -- --home /tmp/okb    # or any directory
npm run serve -- --daemon           # or a running `cua daemon`
npm run serve -- --teleport-home /path/to/generated-home   # teleport from a directory, not the real host
```

## Teleport

Session teleport (`POST /api/teleport/manifest`, then `POST /api/teleport`
with the items you approved) uses the SDK's `space.teleportManifest` and
`space.teleport`. These are tool calls: they work when the server uses the
Cua Spaces daemon (`--daemon`). The embedded runtime refuses them with
`HostCapabilityMissing`, and the server answers 501 with the SDK's message
that teleport ships with Cua Spaces.

App teleport ("Teleport an app…": the app list, the move plan and run, and
window drags) ships with Cua Spaces (source-available), not with the MIT
SDK. The Computer panel's drop zone, the SDK's `<cua-drop-zone>`, still
shows **Send file…** and **Teleport an app…**: files upload (SHA-256
verified), while **Teleport an app…** and an app bundle dropped from Finder
or the Dock show "App teleport ships with Cua Spaces (source-available)."
The `/api/teleport/apps`, `icon`, `plan`, `run` and `window-drags` routes
answer 501 with the same message.

## Routines and group chats

A routine is a saved prompt plus a schedule (every N minutes, daily at, weekly on) that gives one Bot another turn; a group chat sends one message to two to six Bots and shows each reply under its Bot. Both are the SDK's `RoutineStore` and `GroupChatStore` (`@trycua/cua/spaces/routines`, `/groups`) over this app's `BotStore` (`src/core/coworkers.ts`). Routines live in `<home>/routines.json` and fire while the server runs; a Bot mid-turn refuses its slot. `--model-url URL [--model NAME] [--key-var VAR]` points every Bot at a custom model endpoint.

## Picture in picture

The PiP button on the stream pops the desktop out; each row of the window
list pops out that one window (its own media session, closed with the PiP).
Both use `PictureInPicture` from `@trycua/cua/spaces/pip`: the Document
Picture-in-Picture API when the browser has it, video picture-in-picture
otherwise. Browsers allow one PiP at a time, so a new one replaces the last.

For UI work, `npm run dev:web` serves the page through Vite with `/api` and
`/events` proxied to the server on port 4780 (`OPENKOALABOTS_SERVER`
overrides); open `http://127.0.0.1:5173/#token=<the printed token>`.

## What is in it

| Part | Where | What |
|---|---|---|
| App model | `src/core/app.ts` | One Space, one long-lived agent thread per Bot, a roster from one `agentList` per tick, per-turn transcripts folded from the run's structured events by the SDK's `AgentTranscript`, refusal (never a queue) while a Bot is mid-turn, verified attachments, session teleport behind an approval callback |
| New Space | `src/core/plan.ts` | The wizard's plan to one SDK call (below) |
| Wire | `src/core/wire.ts`, `src/core/webmedia.ts` | Media wire v2 framing (`libs/cua/proto/MEDIA.md`), control messages, Annex B keyframes, `OpenMedia` over the wasm gRPC-Web client |
| Server | `src/server/` | Loopback HTTP + WebSocket, random bearer, one poll loop, stream **tickets** (the spacesd token never reaches the page), session teleport only for a manifest the page showed, app teleport routes that say it ships with Cua Spaces |
| Page pieces | `src/ui/` | DOM-only building blocks shared by the page and the tests: the image dropdown and wizard, thread cards, the Space section, friendly errors |
| Page | `web/` | Sidebar (New Bot, search, roster, Space picker), the thread with the Bot's messages as prose, its activity (install, tools, thinking, turn ends) as muted collapsible groups, approval and file cards and a docked composer, the Computer panel (live desktop decoded by WebCodecs, picture in picture, window list with each app's icon from the Space, drop zone), light and dark |
| Scenario runner | `src/scenario/` | Runs `../openkoalabot-example-scenario/scenario.json` headlessly |

## Spaces

**New Space** opens a four-step wizard (System, Resources, Options,
Summary). The image dropdown lists the `published` entries of the shared
image list, and each entry names the engine it runs on in each location, so
the wizard never offers a combination the image cannot run. Create posts the
plan to `POST /api/spaces/create`, which makes one SDK call,
`spaces.create({ on, image, kind, runtime, name, wait: true, ... })`:

| Where | `on` | `kind` / `runtime` |
|---|---|---|
| This machine | `local` | container / `auto` (gVisor when available), vm / `qemu`, vm / `lume`; plus `cpus`, `memoryMb`. The SDK runs it itself |
| Cua Cloud | `cloud` | container / `gvisor`, vm / `kubevirt`. Needs `cua auth login` (or `CUA_CLIENT_ID`/`CUA_CLIENT_SECRET`); metered until deleted |

**Add by address** (wizard footer or empty state) registers a machine that
already runs cua-spacesd. **Delete…** is a separate control that always asks
first: a created Space is deleted with its sandbox, one added by address is
only removed from the list.

## Tests

```sh
npm test          # build + node --test test/*.test.mjs
npm run typecheck # server and page
```

| File | Covers |
|---|---|
| `ui.test.mjs` | under happy-dom: the dropdown equals the published list, the wizard (disabled targets, one plan), Delete needs confirmation, thread cards, friendly errors |
| `computer.test.mjs` | under happy-dom: the window list (one line per window, a PiP button each) and the one drop zone |
| `plan.test.mjs` | plan to `spaces.create` options (location, kind, engine), refusals, defaults |
| `server.test.mjs` | loopback + bearer, the whole API against a fake Space, `spaces/create`, window list and per-window stream tickets, the event socket |
| `app.test.mjs` | thread lifecycle, refusal, degraded probes, the one-call roster, verified attachments, teleport consent |
| `teleport.test.mjs` | the app teleport routes answer that it ships with Cua Spaces; the embedded runtime's session teleport refusal (`HostCapabilityMissing`) reaches the page as that message |
| `wire.test.mjs`, `webmedia.test.mjs` | framing, codec string, `OpenMedia`, ticket subprotocol |
| `spec.test.mjs` | the shared scenario parses |
| `web-client.test.mjs`, `fixture-lane.test.mjs` | the stream path and the whole scenario against `cua-test-fixtures` |

The last two need `cua-test-fixtures` (`libs/cua/scripts/build-test-fixtures.sh`,
or point `CUA_TEST_FIXTURES` at a build) and skip without it.

## The scenario

```sh
npm run build
../openkoalabot-example-scenario/run.sh --impl ts --lane fixture   # loopback spacesd core
../openkoalabot-example-scenario/run.sh --impl ts --lane docker    # a linux container (gVisor, 4 GiB)
```

The runner embeds the MIT runtime, so the `teleport` step skips with the
SDK's reason (teleport ships with Cua Spaces).

The same behaviour ships in [`openkoalabot-example-swift`](../openkoalabot-example-swift)
and [`openkoalabot-example-tauri`](../openkoalabot-example-tauri); all three pass the
shared scenario in [`openkoalabot-example-scenario`](../openkoalabot-example-scenario).
