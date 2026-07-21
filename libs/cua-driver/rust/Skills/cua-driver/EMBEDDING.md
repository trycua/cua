# Embedding cua-driver in your agent harness without introducing new permissions

This guide is for teams shipping a macOS app (an "agent harness") that wants
cua-driver's background computer-use and agent-cursor overlay **inside their
own app**, without shipping a second app bundle and without their users ever
seeing a second macOS permission prompt. Your app requests Accessibility and
Screen Recording once; the embedded driver inherits those grants.

A working reference host lives in the cua repo at
`libs/cua-driver/rust/examples/embedded-host-macos/`
(https://github.com/trycua/cua). This doc ships standalone in the skill
pack, so the path is given rather than a relative link.

## How macOS attributes these permissions (what you must know)

macOS TCC (the privacy system behind System Settings → Privacy & Security)
does not attribute Accessibility or Screen Recording to an executable path.
It attributes them to the **responsible process**: the app at the top of the
process's launch chain, as tracked by the kernel/LaunchServices. When your
signed app spawns a child with `posix_spawn`, `NSTask`/`Process`, or plain
`fork`/`exec`, that child stays inside *your* responsibility chain — TCC
checks made by the child are answered with **your app's** grants, and any
prompt it triggered would name **your app**. This is exactly the behavior
embedding relies on: grant once to the host, and every well-behaved child
inherits. (Apple documents the attribution chain; you can watch it live with
`log stream --debug --predicate 'subsystem == "com.apple.TCC" AND eventMessage BEGINSWITH "AttributionChain"'`.)

Two things break the chain, and both are things the embedded driver must
*not* do (and, in embedded mode, does not do). First, launching via
LaunchServices (`open -a …`, `NSWorkspace.open`) makes the launched app its
own responsible process. Second, a process can explicitly *disclaim*
responsibility for a child (`responsibility_spawnattrs_setdisclaim`), making
the child its own responsible process — standalone cua-driver does this on
purpose so its permissions attach to a stable `com.trycua.driver` identity
instead of whatever terminal launched it. Embedded mode turns that off.

Note this is TCC **responsibility** inheritance — it is unrelated to App
Sandbox inheritance (`com.apple.security.inherit`). This guide assumes a
non-sandboxed host, which is typical for agent harnesses; a sandboxed host
spawning a non-sandboxed helper raises separate App Sandbox questions that
embedded mode does not address.

## Launching in embedded mode

```sh
# env var form — set by the host on the child process
CUA_DRIVER_EMBEDDED=1 CUA_DRIVER_HOST_BUNDLE_ID=com.yourco.yourapp \
  cua-driver serve --socket /tmp/yourapp-cua.sock

# after the daemon socket is ready, start the stdio MCP proxy
CUA_DRIVER_EMBEDDED=1 cua-driver mcp --socket /tmp/yourapp-cua.sock
```

Requirements on the host side:

- **Spawn `cua-driver serve --embedded` directly** as a child process
  (`Process`/`NSTask`, `posix_spawn`, `exec` from your own code). Do
  **not** launch the daemon via `open(1)` or `NSWorkspace` — that hands it
  to LaunchServices and breaks inheritance.
- Give the daemon a private socket and wait until it is accepting connections.
- Spawn `cua-driver mcp --embedded --socket <path>` and speak MCP over that
  proxy's stdin/stdout (line-delimited JSON-RPC). The proxy never executes
  tools; the host-owned daemon does.
- Request Accessibility and Screen Recording **from your app** before (or
  after — the driver just reports "not granted" until then) starting the
  driver, using `AXIsProcessTrustedWithOptions([kAXTrustedCheckOptionPrompt: true])`
  and `CGRequestScreenCaptureAccess()`.

Only the exact value `CUA_DRIVER_EMBEDDED=1` enables embedded mode; anything
else is ignored (fail-safe). `--host-bundle-id` is an advisory label echoed
in `check_permissions` output and logs — it is **not** a trust signal; trust
comes from the OS responsibility chain, so there is nothing to spoof by
setting it.

## Choosing the agent permission mode

Embedded hosts own their user-facing permission experience, but they must
select Cua Driver's immutable daemon mode at trusted launch. The choices are:

- `standard`: protected runtime approval for migrated sensitive operations.
- `bounded`: unattended work inside an approved, exact session manifest.
- `unrestricted`: no Cua runtime approvals; use only when the host accepts the
  consequences of prompt injection and unintended actions.

For unrestricted embedding, use the explicit two-part environment contract:

```sh
CUA_DRIVER_EMBEDDED=1 \
CUA_DRIVER_PERMISSION_MODE=unrestricted \
CUA_DRIVER_DANGEROUSLY_BYPASS_APPROVALS=1 \
  cua-driver serve --embedded --socket /tmp/yourapp-cua.sock
```

Both values are required and contradictory values fail before the daemon
binds. They belong in trusted launcher configuration; never expose either as
an agent-settable MCP or raw-socket argument. Interactive operators can use
the equivalent single CLI shortcut, `--dangerously-bypass-approvals`, which
selects unrestricted mode and records the acknowledgement. The older
`autonomous` mode name remains accepted as an alias for `bounded` during
migration.

### Node and Electron hosts

Use `@trycua/cua-driver-embedded` instead of implementing process and socket
management in every host. The package starts a private daemon directly, waits
until its socket accepts connections, returns the MCP configuration, and owns
restart and cleanup:

```ts
import { EmbeddedCuaDriver } from '@trycua/cua-driver-embedded';

const driver = new EmbeddedCuaDriver({
  binaryPath: '/path/inside/YourApp.app/cua-driver',
  hostBundleId: 'com.example.your-app',
});
const connection = await driver.start();
// Register connection.mcp with the host's MCP client.
await driver.stop();
```

Electron main processes can use the package's `/electron` entry point to
request Accessibility and check Screen Recording after `app.whenReady()`.
Some macOS releases refuse to raise a Screen Recording prompt; in that case,
open the Screen Recording settings pane with
`openMacOSScreenRecordingSettings()`, ask the user to add the host app, and
start the driver only after both checks return true.

## What embedded mode changes (and what it doesn't)

|                                | Standalone                          | Embedded (`CUA_DRIVER_EMBEDDED=1`)       |
| ------------------------------ | ----------------------------------- | ---------------------------------------- |
| Responsibility disclaim re-exec| ON (owns its TCC identity)          | OFF (stays in the host's chain)          |
| Tool execution process          | `serve` daemon                     | host-spawned `serve --embedded` daemon |
| Daemon auto-relaunch via `open -a CuaDriver` | Yes, when installed   | Never (would leave the host's chain)     |
| TCC identity                   | `com.trycua.driver`                 | the host app                             |
| Permission prompts / startup gate | May prompt once                  | **Never prompts**                        |
| Settings → Privacy & Security entries | CuaDriver                    | your app only                            |
| `check_permissions` `source.attribution` | `driver-daemon` (or `caller`) | `host`                            |
| Overlay, background input, capture, all tools | full               | full — identical                          |

Everything else — the agent-cursor overlay, background (no-focus-steal)
clicking and typing, AX tree reads, per-window screenshots — is unchanged.
When embedded mode is off, nothing in this feature is active: standalone
behavior is byte-for-byte what it was.

## The responsibility-chain requirement, exactly

The host must be the responsible process for the driver. That holds
automatically when you spawn the `serve` daemon directly and embedded mode
is on. If the daemon were allowed to disclaim (standalone behavior), macOS
would treat it as its own responsible process: your user would get a *second* prompt
attributed to the driver binary, a second Settings entry, and capture/AX
would fail until that second grant — the exact experience embedding exists
to eliminate. Embedded mode short-circuits the disclaim re-exec
(`responsibility.rs`) and the `open -a CuaDriver` daemon relaunch. MCP is
always a proxy, so the embedded daemon remains the single process that checks
TCC and executes tools.

### App + gateway architectures

`--embedded` does not transfer a GUI app's permissions to the driver; it
only keeps the driver inside its **spawner's** TCC responsibility chain. If
your product has a GUI app that owns the macOS grants and a separate
gateway, daemon, or Node process that spawns MCP servers, registering
`cua-driver serve --embedded` with the gateway makes the daemon inherit the
gateway's identity, not the app's. Spawn the daemon from the GUI app itself;
gateways may connect an MCP proxy to the app-owned private socket.

```text
Wrong (inherits the gateway's identity):        Right:

gateway / node daemon                           YourApp.app
  └─ cua-driver serve --embedded                  ├─ cua-driver serve --embedded
                                                  └─ cua-driver mcp --embedded
                                                     --socket <private-path>
```

Note `check_permissions` cannot detect this: `source.attribution` reports
`host` whenever `CUA_DRIVER_EMBEDDED=1` is set, even if a gateway spawned
the driver. The symptoms are grant booleans that track the *gateway's* TCC
state and prompts/Settings entries naming the gateway process; see
Troubleshooting below.

## Reading `check_permissions` from the host

Call the `check_permissions` tool over MCP. In embedded mode it never raises
a dialog (the `prompt` argument is ignored) and returns:

```json
{
  "accessibility": true,
  "screen_recording": true,
  "screen_recording_capturable": null,
  "direct_capture_status": "not_checked",
  "source": {
    "attribution": "host",
    "host_bundle_id": "com.yourco.yourapp",
    "embedded": true,
    "pid": 12345,
    "responsible_ppid": 12300,
    "executable": "/path/to/cua-driver",
    "disclaim_env": false,
    "note": "Embedded mode: these booleans reflect the HOST app's TCC grant…"
  }
}
```

- `accessibility` / `screen_recording` — the live TCC state *of your app's
  grant*, answered from inside the driver process (which shares your
  identity). If both are true, it is safe to drive the desktop.
- `screen_recording_capturable` / `direct_capture_status` — embedded
  `check_permissions` is read-only and never runs Tahoe's prompt-capable
  ScreenCaptureKit probe, so these are `null` / `not_checked`. The host owns
  the consent UX and should verify pixels with an explicit screenshot or
  capture operation after explaining the prompt.
- `source.attribution` values:
  - `host` — embedded mode; booleans reflect the host's grant. What you
    should always see when embedding.
  - `driver-daemon` — standalone daemon owning `com.trycua.driver`. If you
    see this while embedding, embedded mode is not actually set.
  - `caller` — a non-embedded, non-bundle launch (e.g. someone ran the
    binary from a terminal); booleans reflect the terminal's grants.

If a permission is missing, the correct reaction is: **the host requests
it** (the two API calls above), then re-calls `check_permissions`. The
driver will never pop its own dialog in embedded mode.

Heads-up on grant timing: macOS caches TCC answers per process. If your app
requests/receives the grants *after* the driver child is already running,
restart the driver child so it re-queries with a fresh cache.

## Minimal host example (copy-paste)

The file below is the complete reference host — mirrored verbatim from
`libs/cua-driver/rust/examples/embedded-host-macos/ExampleAgentHarness.swift`
in the cua repo (which also has a build-and-run `demo.sh` covering the
TCC-reset flow).
It requests the two grants as the host, spawns an embedded daemon plus an MCP
proxy, and runs the whole demo sequence: attribution check, background screenshot,
background AX read, agent-cursor glide.

`ExampleAgentHarness.swift`:

```swift
// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// ExampleAgentHarness — minimal reference host for embedding cua-driver.
// Mirrored verbatim in Skills/cua-driver/EMBEDDING.md ("Minimal host
// example") — keep the two in sync.
//
// Runs the one-grant demo sequence from EMBEDDING.md end to end:
//   1. Requests Accessibility + Screen Recording AS THE HOST (the only
//      prompts the user ever sees), then
//   2. spawns an embedded cua-driver daemon plus its stdio MCP proxy and
//      verifies attribution, takes a background screenshot,
//      reads a background app's window state, and glides the agent-cursor
//      overlay — with zero driver-side prompts.
//
// Launched via `open` (see demo.sh) the app has no terminal, so all
// output also goes to /tmp/cua-embedded-demo.log.

import Foundation
import ApplicationServices
import CoreGraphics

let logPath = "/tmp/cua-embedded-demo.log"
FileManager.default.createFile(atPath: logPath, contents: nil)
let logFile = FileHandle(forWritingAtPath: logPath)!
func log(_ line: String) {
    print(line)
    logFile.write((line + "\n").data(using: .utf8)!)
}

// 1. Request both grants AS THE HOST — the only prompts in the whole flow.
let axOpts = ["AXTrustedCheckOptionPrompt": true] as CFDictionary
let ax = AXIsProcessTrustedWithOptions(axOpts)
let sr = CGRequestScreenCaptureAccess()
log("host grants — accessibility: \(ax), screen recording: \(sr)")
// Keep going even without grants: the run registers BOTH rows in one pass
// (the AX request above, plus — on newer macOS, where the app only appears
// in the Screen Recording pane after a real ScreenCaptureKit attempt — the
// embedded driver's live probe below, registered as THE HOST, which is the
// point of embedding). Grant both in one Settings visit, then re-run.
if !ax || !sr {
    log("after this run: grant the missing item(s) in System Settings, then re-run")
}

// 2. Spawn the daemon as a DIRECT child (never via `open`/NSWorkspace —
//    that breaks responsibility inheritance), then attach an MCP proxy.
let driverPath = ProcessInfo.processInfo.environment["CUA_DRIVER_PATH"]
    ?? "/usr/local/bin/cua-driver"
let socketPath = "/tmp/cua-embedded-\(ProcessInfo.processInfo.processIdentifier).sock"
var env = ProcessInfo.processInfo.environment
env["CUA_DRIVER_EMBEDDED"] = "1"
env["CUA_DRIVER_HOST_BUNDLE_ID"] = Bundle.main.bundleIdentifier ?? ""

let daemon = Process()
daemon.executableURL = URL(fileURLWithPath: driverPath)
daemon.arguments = ["serve", "--embedded", "--socket", socketPath]
daemon.environment = env
daemon.standardOutput = logFile
daemon.standardError = logFile
try daemon.run()

let deadline = Date().addingTimeInterval(10)
while !FileManager.default.fileExists(atPath: socketPath) && Date() < deadline {
    Thread.sleep(forTimeInterval: 0.05)
}
guard FileManager.default.fileExists(atPath: socketPath) else {
    log("embedded daemon did not create \(socketPath)")
    daemon.terminate()
    exit(1)
}

let driver = Process()
driver.executableURL = URL(fileURLWithPath: driverPath)
driver.arguments = ["mcp", "--embedded", "--socket", socketPath]
driver.environment = env
let toDriver = Pipe(), fromDriver = Pipe()
driver.standardInput = toDriver
driver.standardOutput = fromDriver
try driver.run()

// 3. Line-delimited JSON-RPC 2.0 over the child's stdio.
var buffer = Data()
func send(_ obj: [String: Any]) {
    var data = try! JSONSerialization.data(withJSONObject: obj)
    data.append(0x0A)
    toDriver.fileHandleForWriting.write(data)
}
func readMessage() -> [String: Any] {
    while true {
        if let nl = buffer.firstIndex(of: 0x0A) {
            let line = buffer.subdata(in: buffer.startIndex..<nl)
            buffer.removeSubrange(buffer.startIndex...nl)
            if line.isEmpty { continue }
            return (try? JSONSerialization.jsonObject(with: line)) as? [String: Any] ?? [:]
        }
        let chunk = fromDriver.fileHandleForReading.availableData
        if chunk.isEmpty { log("driver exited unexpectedly"); exit(1) }
        buffer.append(chunk)
    }
}
var nextId = 0
func call(_ tool: String, _ args: [String: Any] = [:]) -> [String: Any] {
    nextId += 1
    send(["jsonrpc": "2.0", "id": nextId, "method": "tools/call",
          "params": ["name": tool, "arguments": args]])
    while true {
        let msg = readMessage()
        if msg["id"] as? Int == nextId {
            if let error = msg["error"] as? [String: Any] {
                log("RPC error for \(tool): \(error)")
            }
            return msg["result"] as? [String: Any] ?? [:]
        }
    }
}

nextId += 1
send(["jsonrpc": "2.0", "id": nextId, "method": "initialize", "params": [
    "protocolVersion": "2024-11-05", "capabilities": [:],
    "clientInfo": ["name": "ExampleAgentHarness", "version": "0.1"]]])
_ = readMessage()
send(["jsonrpc": "2.0", "method": "notifications/initialized"])
log("embedded cua-driver daemon + proxy started (\(driverPath)) — no driver prompt should have appeared")

// 4. check_permissions must report attribution "host" and never prompt.
let perms = call("check_permissions")
let structured = perms["structuredContent"] as? [String: Any] ?? [:]
let source = structured["source"] as? [String: Any] ?? [:]
let attribution = source["attribution"] as? String ?? "?"
log("check_permissions — attribution: \(attribution) (want: host), " +
    "capturable: \(structured["screen_recording_capturable"] ?? "?")")

// 5. Background AX read + window screenshot — proves both grants
//    inherited without focusing anything. launch_app resolves pid +
//    windows without foregrounding; get_window_state returns the AX
//    element tree AND a screenshot of the (background) window.
let launch = call("launch_app", ["bundle_id": "com.apple.finder"])
let launched = launch["structuredContent"] as? [String: Any] ?? [:]
let pid = launched["pid"] as? Int ?? 0
let windows = launched["windows"] as? [[String: Any]] ?? []
let windowId = windows.first?["window_id"] as? Int ?? 0
log("launch_app(Finder) — pid: \(pid), windows: \(windows.count)")

let state = call("get_window_state", ["pid": pid, "window_id": windowId])
let images = (state["content"] as? [[String: Any]] ?? [])
    .filter { $0["type"] as? String == "image" }
let hasTree = (state["structuredContent"] as? [String: Any])?["elements"] != nil
log("get_window_state(Finder) — tree: \(hasTree ? "ok" : "EMPTY"), " +
    "screenshot: \(images.count) image(s) (want: ≥1)")

// 6. Agent-cursor glide — shows the overlay, no real-pointer move.
log("watch the agent cursor glide now (no real-pointer move)…")
let cursor1 = call("move_cursor", ["x": 200, "y": 200])
Thread.sleep(forTimeInterval: 2)
let cursor2 = call("move_cursor", ["x": 900, "y": 500])
Thread.sleep(forTimeInterval: 2)
let cursorOk = (cursor1["isError"] as? Bool) != true &&
    (cursor2["isError"] as? Bool) != true
log("move_cursor — \(cursorOk ? "ok" : "FAILED")")

let pass = attribution == "host" && !images.isEmpty && hasTree && cursorOk
log(pass ? "DEMO COMPLETE: PASS" : "DEMO COMPLETE: FAIL")
driver.terminate()
daemon.terminate()
exit(pass ? 0 : 1)
```

Build it as a signed app bundle (a stable signing identity is what keys
the TCC grant rows to your app):

```sh
mkdir -p ExampleAgentHarness.app/Contents/MacOS
swiftc -O ExampleAgentHarness.swift \
  -o ExampleAgentHarness.app/Contents/MacOS/ExampleAgentHarness \
  -framework ApplicationServices
printf '%s\n' '<?xml version="1.0" encoding="UTF-8"?>' \
  '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">' \
  '<plist version="1.0"><dict>' \
  '<key>CFBundleExecutable</key><string>ExampleAgentHarness</string>' \
  '<key>CFBundleIdentifier</key><string>com.trycua.example-agent-harness</string>' \
  '<key>CFBundlePackageType</key><string>APPL</string>' \
  '</dict></plist>' > ExampleAgentHarness.app/Contents/Info.plist
codesign --force --sign - ExampleAgentHarness.app   # use your Developer ID in production
open ExampleAgentHarness.app   # `open` is correct HERE: the HOST must be its own responsible process
tail -f /tmp/cua-embedded-demo.log
```

## Troubleshooting

**"I still get a second permission prompt / a second Settings entry."**
Embedded mode is not in effect for the process doing the TCC check. Causes,
in order of likelihood: (a) `CUA_DRIVER_EMBEDDED` is not exactly `1`, or was
set on your app but not passed into the daemon child's environment; (b) the daemon
was launched via `open(1)` / `NSWorkspace` instead of spawned directly, so
it is its own responsible process; (c) the MCP proxy connected to an old standalone `CuaDriver.app` daemon — check
`check_permissions` → `source.attribution` (must be `host`) and verify
that the proxy uses the host's private socket. To see exactly which identity macOS is charging,
run: `log stream --debug --predicate 'subsystem == "com.apple.TCC" AND
eventMessage BEGINSWITH "AttributionChain"'` and trigger the action again.

**"Screenshots come back black even though `screen_recording: true`."**
The read-only permission check cannot verify direct ScreenCaptureKit access
without risking a system dialog. Exercise an explicit screenshot only after
the host has explained and requested consent. If that fails, the grant may not
belong to the driver's current responsible identity, may have been reset, or
the driver may have escaped the host's chain (see the previous item). Restart
the driver child after any grant change — TCC answers are cached per process.

**"The AX tree comes back empty / clicks do nothing."**
`AXIsProcessTrusted()` is false for the effective identity. The host hasn't
been granted Accessibility, or was granted it *after* the driver child
started (per-process cache again — restart the child), or the app was
re-signed/moved so the existing grant row no longer matches it (remove and
re-add it in System Settings, or `tccutil reset Accessibility <your-bundle-id>`
and re-grant).

**"It worked, then stopped after I updated/re-signed my app."**
TCC grant rows are keyed to the app's code-signing identity. A signature
change can orphan the old row. Reset and re-grant:
`tccutil reset Accessibility com.yourco.yourapp && tccutil reset ScreenCapture com.yourco.yourapp`.

## Platform notes (Windows / Linux)

Embedding also works on Windows and Linux (X11) with **no per-app permission
ceremony**. The host still spawns a daemon in the intended interactive session
or desktop, then points MCP and CLI adapters at its private socket. The
one-grant inheritance story in this guide is macOS-specific because macOS is
the platform where Accessibility and Screen Recording grants follow the
responsibility chain.

Two known exceptions:

- **Windows, elevated / UWP targets**: injecting into higher-integrity
  windows needs the uiAccess-signed worker (`cua-driver-uia`). An embedded
  host that must drive elevated apps has to manage that worker and connect
  clients to its named pipe.
- **Linux Wayland** (compositor-specific): capture goes through XDG desktop
  portals, which prompt per-session at capture time and cannot be
  pre-granted by the host. X11 has no portal gate.
