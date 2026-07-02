// ExampleAgentHarness — minimal reference host for embedding cua-driver.
// Mirrored verbatim in Skills/cua-driver/EMBEDDING.md ("Minimal host
// example") — keep the two in sync.
//
// Runs the one-grant demo sequence from EMBEDDING.md end to end:
//   1. Requests Accessibility + Screen Recording AS THE HOST (the only
//      prompts the user ever sees), then
//   2. spawns cua-driver as a direct child in embedded mode and, over
//      stdio MCP, verifies attribution, takes a background screenshot,
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

// 2. Spawn cua-driver as a DIRECT child (never via `open`/NSWorkspace —
//    that breaks responsibility inheritance) in embedded mode.
let driverPath = ProcessInfo.processInfo.environment["CUA_DRIVER_PATH"]
    ?? "/usr/local/bin/cua-driver"
let driver = Process()
driver.executableURL = URL(fileURLWithPath: driverPath)
driver.arguments = ["mcp"]
var env = ProcessInfo.processInfo.environment
env["CUA_DRIVER_EMBEDDED"] = "1"
env["CUA_DRIVER_HOST_BUNDLE_ID"] = Bundle.main.bundleIdentifier ?? ""
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
log("embedded cua-driver started (\(driverPath)) — no driver prompt should have appeared")

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
_ = call("move_agent_cursor", ["x": 200, "y": 200])
Thread.sleep(forTimeInterval: 2)
_ = call("move_agent_cursor", ["x": 900, "y": 500])
Thread.sleep(forTimeInterval: 2)

let pass = attribution == "host" && !images.isEmpty && hasTree
log(pass ? "DEMO COMPLETE: PASS" : "DEMO COMPLETE: FAIL")
driver.terminate()
exit(pass ? 0 : 1)
