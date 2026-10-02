# Samples

| Sample | What it is |
|---|---|
| [`cua-bots-macos`](cua-bots-macos) | Cua Bots for macOS: persistent bots, each with a koala face, its own Space, a memory in the Cua Volume layout, routines, approvals, custom rules and saved sign-ins. SwiftUI on the Cua Swift SDK and the Cua Spaces app export. The featured example; source-available (FSL-1.1-MIT) |
| [`cua-bots-ios`](cua-bots-ios) | Cua Bots for iPhone: a remote client for the Mac's bots; chat, watch and take over their computers, answer approvals, get notifications. Source-available (FSL-1.1-MIT) |
| [`openkoalabot-example-swift`](openkoalabot-example-swift) | OpenKoalaBots, a chat app for agent coworkers on Cua Spaces: a SwiftUI app on the `Cua` package via the Swift overlay [`libs/spaces-sdk-swift`](../libs/spaces-sdk-swift) and `CuaSpacesStreaming` ([`libs/spaces-app-swift`](../libs/spaces-app-swift)). The first of the three; its 209-test suite and `RUBRIC.md` are the behaviour spec. Source-available (FSL-1.1-MIT) |
| [`openkoalabot-example-tauri`](openkoalabot-example-tauri) | The same app as a Rust core on the `cua-spaces` crate, with a Tauri 2 + React shell |
| [`openkoalabot-example-ts`](openkoalabot-example-ts) | The same app in TypeScript on `@trycua/cua`: a Node agent loop, a local server, and a web UI that streams through `@trycua/cua/browser` (gRPC-Web + the ticketed `/media` socket) and WebCodecs |
| [`openkoalabot-example-scenario`](openkoalabot-example-scenario) | The shared, language-neutral scenario all three run headlessly: add or claim a Space, stream, run an agent thread (fake CLI), send a file, teleport a generated profile, run presence with two clients, release |
| [`infinite-canvas-swift`](infinite-canvas-swift) | infinite-canvas: every window of every Space on one zoomable canvas, with agent and coworker cursors and agent threads. SwiftUI and AppKit on the Cua Swift packages. Source-available (FSL-1.1-MIT) |
| [`driver`](driver) | cua-driver samples |
| [`python`](python) | Python samples |

Run all three OpenKoalaBots samples against the same Space kinds:

```sh
samples/openkoalabot-example-scenario/run.sh --impl all --lane fixture   # hermetic
samples/openkoalabot-example-scenario/run.sh --impl all --lane docker    # linux under gVisor, one container at a time
```

Each implementation's README has its build, test and run commands.
