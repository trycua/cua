import Foundation
@testable import CuaSpaces

/// An in-process Spaces backend.
///
/// Every tool the SDK models, answering with the **shapes the Rust
/// `cua-spaces` server answers with** (`CuaBackedTests` drives the same
/// overlay against the real one, so a drift here fails there), including the ones `FRICTION.md` records as awkward:
/// `isError` results that are not JSON-RPC errors (§3), an `agent_message`
/// reply whose two branches share no key (§4), a `list_spaces` that answers
/// prose when there is nothing to list (§4), a window list that spells the
/// identifier `window` (§7), and `phase` strings that differ per provider (§6).
///
/// A fake that answered nicely would test nothing.
actor FakeSpacesBackend: SpacesTransport {

    struct Run {
        var agent = "claude-code"
        var status = "running"
        var reason = "the turn is in flight"
        var acceptsMessage = false
        var summary = ""
        var outputTail = "line 1\nline 2"
        var exitCode: Int?
        var pid = 4242
    }

    var spaces: [[String: JSONValue]] = [
        ["id": "local:cua-space-test", "provider": "local", "os": "macos", "phase": "running",
         "ip": "192.168.64.2"],
        ["id": "cloud:space-abc", "provider": "cloud", "os": "linux", "phase": "Bound"],
        ["id": "cloud:space-dead", "provider": "cloud", "os": "linux", "phase": "Failed"],
    ]
    var runs: [String: Run] = [:]
    var windows: [[String: JSONValue]] = [
        ["window": "target-aaa", "app_name": "Terminal", "title": "lume — watch.command",
         "pid": 4242, "visible": true,
         "geometry": .object(["width_px": 1200, "height_px": 800, "scale_factor": 2])],
        ["window": "target-bbb", "app_name": "Safari", "title": "crm.acme.com",
         "pid": 99, "visible": true,
         "geometry": .object(["width_px": 1440, "height_px": 900, "scale_factor": 2])],
    ]
    /// A real filesystem, so `upload` collisions are real collisions.
    var files: Set<String> = []
    var uploadedBytes: [String: String] = [:]
    /// Every tool name called, in order — the evidence for "one poll per tick".
    private(set) var calls: [String] = []
    /// Set to make the next call of a named tool fail the way the server fails.
    var failNext: [String: String] = [:]
    /// Answer `list_spaces` with the prose sentence instead of an array.
    var listSpacesAnswersProse = false
    /// What `show` the last `agent_start` carried, `nil` if it carried none.
    /// A run started with `show: false` opens no Terminal window, which is the
    /// only cleanup that cannot go wrong.
    private(set) var lastAgentStartShow: Bool?
    /// Which Space the host Mac is currently sharing its network with.
    private(set) var hotspotSpace: String?
    /// What the last `teleport_app` actually asked to transfer — the evidence
    /// that "no selection" means the server's default set and not everything.
    private(set) var lastTeleportInclude: [String] = []
    /// The arguments the last `create_space` carried: the evidence that where
    /// a Space runs is always sent, never left to a default.
    private(set) var lastCreateArguments: [String: JSONValue]?
    private(set) var lastTeleportApp: String?
    /// Whether the last `teleport_app` carried the sensitive-item
    /// acknowledgement the Rust server requires.
    private(set) var lastTeleportAcknowledged = false

    func callCount(_ tool: String) -> Int { calls.filter { $0 == tool }.count }
    func resetCalls() { calls = [] }

    /// Leave a Terminal window behind whose title names a run, the way a real
    /// Space does when the `osascript` close silently matched nothing.
    func leaveWindow(titled title: String, id: String = "target-orphan") {
        windows.append(
            ["window": .string(id), "app_name": "Terminal", "title": .string(title),
             "pid": 4242, "visible": true,
             "geometry": .object(["width_px": 900, "height_px": 500, "scale_factor": 1])])
    }

    func startRun(_ id: String, _ run: Run) { runs[id] = run }

    /// A run this SDK did not start — work already happening in the Space.
    func addForeignRun(_ id: String) { runs[id] = Run(summary: "someone else's work") }

    func setOutput(of id: RunID, to text: String) {
        setRun(id.rawValue) { $0.outputTail = text }
    }

    func setAcceptsMessage(_ id: RunID, _ value: Bool) {
        setRun(id.rawValue) { $0.acceptsMessage = value }
    }

    func finish(_ id: RunID) {
        setRun(id.rawValue) {
            $0.status = "finished"
            $0.reason = "the process exited"
            $0.exitCode = 0
        }
    }

    func setRun(_ id: String, _ transform: (inout Run) -> Void) {
        guard var r = runs[id] else { return }
        transform(&r)
        runs[id] = r
    }

    func callTool(_ name: String, _ arguments: [String: JSONValue]) async throws -> JSONValue {
        calls.append(name)
        if let message = failNext.removeValue(forKey: name) {
            // Exactly how the real server fails: a *successful* result carrying
            // isError, which the transport must turn into a throw (§3).
            throw SpacesError.toolFailed(tool: name, message: "error: \(message)")
        }
        switch name {
        case "list_spaces":
            if listSpacesAnswersProse {
                return .string("No Spaces. Use create_space to make one.")
            }
            return .array(spaces.map { JSONValue.object($0) })

        case "create_space":
            // Shaped like the Rust server: the registered Space with
            // `phase: "ready"`, or `{id, phase: "starting"}` with wait=false.
            lastCreateArguments = arguments
            let on = arguments["on"]?.stringValue ?? "local"
            guard on == "local" || on == "cloud" else {
                throw SpacesError.toolFailed(
                    tool: name, message: "invalid placement: unknown location \(on); valid on: local, cloud")
            }
            let id = "\(on):\(arguments["name"]?.stringValue ?? "space-\(UUID().uuidString.prefix(6))")"
            let os = on == "local" ? "macos" : "linux"
            if arguments["wait"]?.boolValue == false {
                spaces.append(["id": .string(id), "provider": .string(on), "os": .string(os),
                               "phase": "starting"])
                return .object(["id": .string(id), "phase": "starting"])
            }
            let row: [String: JSONValue] = ["id": .string(id), "provider": .string(on),
                                            "os": .string(os), "phase": "ready"]
            spaces.append(row)
            var result = row
            result["reused"] = .bool(false)
            return .object(result)

        case "delete_space":
            let id = arguments["space"]?.stringValue ?? ""
            let provider = spaces.first { $0["id"]?.stringValue == id }?["provider"]?.stringValue
            spaces.removeAll { $0["id"]?.stringValue == id }
            return .string(provider == "direct" ? "Removed \(id) (added by address)" : "Deleted \(id)")

        case "stop_space", "start_space":
            let id = arguments["space"]?.stringValue ?? ""
            let on = name == "start_space"
            return .object([
                "space": .string(id),
                "state": .string(on ? "running" : "suspended"),
                "power": "suspend",
                "message": .string(on ? "Resumed \(id)." : "Suspended \(id) (its memory is kept)."),
            ])

        case "agent_start":
            lastAgentStartShow = arguments["show"]?.boolValue
            let id = "run-\(UUID().uuidString.prefix(8))"
            runs[id] = Run(summary: arguments["prompt"]?.stringValue ?? "")
            files.insert(Self.expand("~/.cua/agents/\(id)"))
            files.insert("~/Library/LaunchAgents/com.trycua.agentrun.\(id).plist")
            return .object([
                "run_id": .string(id),
                "agent": .string(arguments["agent"]?.stringValue ?? "claude-code"),
                "space": arguments["space"] ?? .null,
                "capabilities": .object(["turn_model": "single_shot", "accepts_followups": true]),
                "notes": .array([.string("auto-approved")]),
            ])

        case "agent_list":
            // §22: the roster feed carries no `reason`.
            return .object(["runs": .array(runs.map { id, r in
                .object(["run_id": .string(id), "agent": .string(r.agent),
                         "status": .string(r.status), "summary": .string(r.summary),
                         "accepts_message": .bool(r.acceptsMessage),
                         "created_at": .number(1_700_000_000)])
            })])

        case "agent_status":
            let id = arguments["run_id"]?.stringValue ?? ""
            guard let r = runs[id] else {
                return .object(["run_id": .string(id), "status": "unknown",
                                "reason": .string("no run record for \(id)"),
                                "accepts_message": false])
            }
            let want = arguments["tail"]?.intValue ?? 80
            let lines = r.outputTail.split(separator: "\n", omittingEmptySubsequences: false)
            let tail = lines.suffix(want).joined(separator: "\n")
            var out: [String: JSONValue] = [
                "run_id": .string(id), "agent": .string(r.agent), "status": .string(r.status),
                "reason": .string(r.reason), "accepts_message": .bool(r.acceptsMessage),
                "summary": .string(r.summary), "output_tail": .string(tail),
            ]
            if let code = r.exitCode { out["exit_code"] = .number(Double(code)) }
            return .object(out)

        case "agent_message":
            let id = arguments["run_id"]?.stringValue ?? ""
            guard let r = runs[id] else {
                throw SpacesError.toolFailed(tool: name, message: "error: no run \(id)")
            }
            let force = arguments["force"]?.boolValue ?? false
            if r.acceptsMessage || force {
                // §4: the delivered branch says "note".
                return .object(["delivered": true, "run_id": .string(id),
                                "note": "resumed the session"])
            }
            // §4: the refused branch says "reason", and shares no key with the
            // delivered one beyond `delivered` and `run_id`.
            return .object(["delivered": false, "run_id": .string(id),
                            "status": .string(r.status),
                            "reason": "a turn is already in flight"])

        case "agent_events":
            let id = arguments["run_id"]?.stringValue ?? ""
            let events: [JSONValue] = runs[id] == nil ? [] : [
                .object(["seq": 1, "kind": "turn_started", "turn": 1]),
            ]
            return .object(["run_id": .string(id), "status": .string(runs[id]?.status ?? "unknown"),
                            "events": .array(events), "cursor": .number(Double(events.count)),
                            "caught_up": true])

        case "agent_interrupt":
            let id = arguments["run_id"]?.stringValue ?? ""
            return .object(["run_id": .string(id), "interrupted": .bool(runs[id] != nil),
                            "status": .string(runs[id]?.status ?? "unknown")])

        case "agent_stop":
            let id = arguments["run_id"]?.stringValue ?? ""
            guard runs[id] != nil else {
                return .object(["stopped": false, "reason": .string("no run \(id)")])
            }
            runs[id]?.status = "finished"
            runs[id]?.acceptsMessage = true
            return .object(["stopped": true, "alive": false, "reason": "verified by probe"])

        case "list_space_windows":
            return .array(windows.map { JSONValue.object($0) })

        case "upload":
            let dest = arguments["dest"]?.stringValue ?? ""
            let source = arguments["path"]?.stringValue ?? ""
            guard FileManager.default.fileExists(atPath: source) else {
                throw SpacesError.toolFailed(tool: name, message: "error: host path not found")
            }
            // The real tool clobbers silently. The fake does too — that is the
            // point of §13.
            uploadedBytes[dest] = (try? String(contentsOfFile: source, encoding: .utf8)) ?? ""
            files.insert(dest)
            return .object(["uploaded": true])

        case "download":
            let path = arguments["path"]?.stringValue ?? ""
            guard files.contains(path) || uploadedBytes[path] != nil else {
                throw SpacesError.toolFailed(tool: name, message: "error: not found: \(path)")
            }
            let dir = arguments["dest"]?.stringValue ?? NSTemporaryDirectory()
            let landed = (dir as NSString).appendingPathComponent((path as NSString).lastPathComponent)
            try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
            try? (uploadedBytes[path] ?? "").write(toFile: landed, atomically: true, encoding: .utf8)
            return .object(["dest": .string(landed)])

        case "space_bash":
            return .string(runBash(arguments["command"]?.stringValue ?? ""))

        case "stream_endpoint":
            // A minted media session: the ticket is in `ws_url` as well as on
            // its own, and `needs_gateway_headers` says whether a browser can
            // attach without the daemon's bridge.
            return .object([
                "space": "local:cua-space-test",
                "media_session_id": "media-1",
                "ws_url": "ws://192.168.64.2:3211/media?ticket=tkt-123",
                "ticket": "tkt-123",
                "codec": "h264",
                "wire_version": 2,
                "frame_size": .array([1200, 800]),
                "needs_gateway_headers": false,
            ])

        case "add_space":
            let url = arguments["url"]?.stringValue ?? ""
            let id = "space://direct/\(url.replacingOccurrences(of: "http://", with: ""))"
            spaces.append(["id": .string(id), "name": arguments["name"] ?? .string(url),
                           "provider": "direct", "phase": "ready"])
            return .object(["id": .string(id), "provider": "direct", "phase": "ready"])

        case "remove_space":
            let id = arguments["space"]?.stringValue ?? ""
            let removed = spaces.first { $0["id"]?.stringValue == id }
            spaces.removeAll { $0["id"]?.stringValue == id }
            return .object(["removed": removed.map(JSONValue.object) ?? .null])

        case "send_file":
            let source = arguments["path"]?.stringValue ?? ""
            guard FileManager.default.fileExists(atPath: source) else {
                throw SpacesError.toolFailed(tool: name, message: "error: \(source) not found")
            }
            let sub = arguments["target_directory"]?.stringValue ?? ""
            let root = "/Users/lume/Downloads" + (sub.isEmpty ? "" : "/\(sub)")
            let dest = root + "/" + (source as NSString).lastPathComponent
            files.insert(dest)
            return .object(["source": .string(source), "destination": .string(root),
                            "dest": .string(dest), "kind": "file",
                            "files": .array([.object(["path": .string(dest), "size": 1,
                                                      "sha256": "00"])]),
                            "bytes": 1, "verified": true])

        case "show_space_pip", "hide_space_pip", "open_space_viewer", "stream_space_window":
            // The real tools answer with a **sentence** about the operator's
            // own desktop, not a structured result. Nothing to render.
            return .string("Pinned \(arguments["space"]?.stringValue ?? "") on your desktop.")

        case "space_write":
            let path = arguments["path"]?.stringValue ?? ""
            files.insert(Self.expand(path))
            uploadedBytes[path] = arguments["content"]?.stringValue ?? ""
            return .string("wrote \(path) (\(uploadedBytes[path]?.utf8.count ?? 0) bytes)")

        case "list_tools":
            // Shaped like the real answer, including the two things that are
            // easy to flatten and must not be: the *other* services, and the
            // "reachable but advertised nothing" warning.
            let service = arguments["service"]?.stringValue ?? "computer-server"
            let filtered = arguments["name"]?.stringValue
            var tools: [JSONValue] = []
            if service == "unity" {
                // Reachable, ready for nothing.
            } else if filtered == nil {
                tools = [.object(["name": "screenshot", "description": "Take a screenshot"]),
                         .object(["name": "click", "description": "Click at a point"])]
            } else {
                tools = [.object(["name": "screenshot", "description": "Take a screenshot",
                                  "inputSchema": .object(["type": "object"])])]
            }
            var out: [String: JSONValue] = [
                "service": .string(service),
                "count": .number(Double(tools.count)),
                "tools": .array(tools),
                "services": .array(["computer-server", "blender", "unity"]),
            ]
            if service == "unity" {
                out["warning"] = .string("reachable and completed its MCP handshake, but "
                                         + "advertised zero tools. It exists; it is not ready.")
            }
            return .object(out)

        case "call_tool":
            // Multimodal: cua-driver answers with an image *and* text.
            return .array([
                .object(["type": "image", "mimeType": "image/png",
                         "data": .string(Data([0x89, 0x50]).base64EncodedString())]),
                .object(["type": "text",
                         "text": .string("ran \(arguments["tool"]?.stringValue ?? "")")]),
            ])

        case "agent_capabilities":
            return .object([
                "statuses": .array(["running", "awaiting_input", "idle", "finished",
                                    "failed", "crashed", "unknown"]),
                "harnesses": .array([
                    .object(["name": "claude-code", "turn_model": "interactive"]),
                    .object(["name": "codex", "turn_model": "single_shot"]),
                ]),
                "status_classifier": "not configured (JEV_API_KEY unset)",
            ])

        case "hotspot_start":
            hotspotSpace = arguments["space"]?.stringValue
            return .object(["space": .string(hotspotSpace ?? ""), "state": "waiting_for_peer",
                            "hotspot_id": "h-1", "socks_address": "127.0.0.1:1080",
                            "active_connections": 0, "bytes_out": 0, "bytes_in": 0,
                            "served_here": true])

        case "hotspot_stop":
            let stopped = hotspotSpace.map { [JSONValue.string($0)] } ?? []
            hotspotSpace = nil
            return .object(["stopped": .array(stopped)])

        case "request_site_login":
            if arguments["request_id"]?.stringValue == nil {
                return .object(["status": "pending", "request_id": "kv-1",
                                "space": arguments["space"] ?? .null])
            }
            return .object(["status": "filled", "site": "example.test",
                            "username_hint": "a***@example.test", "submitted": true,
                            "page_url": "http://login.example.test:8000/login",
                            "target_id": "bt-1", "tab_id": "tab-1"])

        case "hotspot_status":
            guard let space = hotspotSpace else { return .object(["hotspots": .array([])]) }
            return .object(["hotspots": .array([.object([
                "space": .string(space), "state": "active", "hotspot_id": "h-1",
                "socks_address": "127.0.0.1:1080", "active_connections": 1,
                "bytes_out": 10, "bytes_in": 20, "served_here": true])])])

        case "teleport_manifest":
            // The real manifest, verbatim in shape: one sensitive item checked
            // by default, one large item that is sensitive and deliberately
            // *not* checked, and notes that say why.
            return .object([
                "app": .string(arguments["app"]?.stringValue ?? ""),
                "display_name": .string(arguments["app"]?.stringValue ?? ""),
                "scope": "full",
                "items": .array([
                    .object(["label": "Logged-in session",
                             "relative_path": "claude/.credentials.json",
                             "estimated_bytes": 0, "is_sensitive": true,
                             "is_checked_by_default": true]),
                    .object(["label": "Config", "relative_path": "claude/.claude.json",
                             "estimated_bytes": 123_235, "is_sensitive": false,
                             "is_checked_by_default": true]),
                    .object(["label": "Conversations", "relative_path": "claude/projects/",
                             "estimated_bytes": 935_856_847, "count": 14,
                             "count_noun": "projects",
                             "is_sensitive": true, "is_checked_by_default": false]),
                ]),
                "total_estimated_bytes": 935_980_082,
                "notes": .array([
                    .string("The logged-in session includes an OAuth token."),
                    .string("Conversations are unchecked by default."),
                ]),
            ])

        case "teleport_app":
            lastTeleportInclude = (arguments["include"]?.arrayValue ?? []).compactMap(\.stringValue)
            lastTeleportApp = arguments["app"]?.stringValue
            lastTeleportAcknowledged = arguments["acknowledge_sensitive"]?.boolValue ?? false
            let space = JSONValue.string(arguments["space"]?.stringValue ?? "")
            let app = JSONValue.string(arguments["app"]?.stringValue ?? "")
            // The real tool's two answers: the first call files a Keyvault
            // request; the retry with its id delivers (the fake's user has
            // approved by then).
            guard let request = arguments["request_id"]?.stringValue else {
                return .object([
                    "consent_required": true, "moved": false, "request_id": "req-1",
                    "space": space, "app": app,
                    "message": "Moving this session needs the user's approval.",
                ])
            }
            XCTAssertEqual(request, "req-1")
            return .object([
                "consent_required": false, "moved": true, "space": space, "app": app,
                "items": .array(["item-1"]),
                // This fake's vault holds exactly the selection it was asked for.
                "transferred_paths": .array(lastTeleportInclude.map(JSONValue.string)),
                "import_ids": .array(["cua-kv-1"]), "expires_ms": 1,
            ])

        // --- Persistent agents: shapes of `crate::persistent` records ------
        case "persistent_agent_create", "persistent_agent_remove":
            let agent: JSONValue = .object([
                "name": arguments["name"] ?? "ada", "harness": arguments["agent"] ?? "hermes",
                "space": arguments["space"] ?? "local:cua-space-test", "paused": false,
                "space_state": "running", "saved_ms": 0, "created_ms": 1,
            ])
            return name == "persistent_agent_create" ? agent : .object(["removed": agent])
        case "persistent_agent_list":
            return .object(["agents": .array([.object([
                "name": "ada", "harness": "hermes", "space": "local:cua-space-test",
                "paused": false, "space_state": "running", "run_id": "run-1", "saved_ms": 1700000000000,
            ])])])
        case "persistent_agent_send":
            return .object(["run_id": "run-1", "started": true,
                            "restored": .object(["files": 2, "bytes": 10, "millis": 5])])
        case "persistent_agent_save":
            return .object(["files": 1, "bytes": 4, "unchanged": 1, "removed": 0,
                            "blocked": .array([.array(["memories/leak.md", "GitHub token"])]),
                            "millis": 7])
        case "agent_pause":
            return .object(["stopped_run": "run-1", "space_state": "suspended", "millis": 9])
        case "agent_resume":
            return .object(["space": "local:cua-space-test", "recreated": false, "ready_ms": 11])
        case "routine_add", "routine_set_enabled":
            return .object(["id": "R-1", "botID": arguments["agent"] ?? "ada", "title": "Morning",
                            "prompt": "Plan", "label": "Every day at 8:00 AM",
                            "isEnabled": arguments["enabled"] ?? true])
        case "routine_list":
            return .object(["routines": .array([.object(["id": "R-1", "botID": "ada",
                            "title": "Morning", "prompt": "Plan", "label": "Every day at 8:00 AM",
                            "isEnabled": true])])])
        case "routine_remove":
            return .object(["removed": arguments["id"] ?? "R-1"])
        case "notify_user":
            return .object(["notified": true, "id": "n-1"])
        case "notifications_list":
            return .object(["notifications": .array([.object([
                "id": "n-1", "at_ms": 1700000000000, "agent": "ada", "kind": "turn_ended",
                "title": "ada", "body": "Your research is ready.", "read": false])])])
        case "notifications_ack":
            return .object(["marked": 1])
        case "computer_access_grant":
            return .object(["id": "g-1", "agent": arguments["agent"] ?? "ada",
                            "machine": arguments["machine"] ?? "relay:0123", "created_ms": 1])
        case "computer_access_revoke":
            return .object(["revoked": 1])
        case "computer_access_list":
            return .object(["grants": .array([.object(["id": "g-1", "agent": "ada",
                            "machine": "relay:0123", "created_ms": 1, "revoked": false])])])
        // Cua Volume: one file, one grant, one request; enough to prove each
        // typed call reaches its tool with its arguments.
        case "volume_ls":
            return .object(["path": .string(arguments["path"]?.stringValue ?? ""),
                            "principal": .string(arguments["as_agent"]?.stringValue.map { "agent:\($0)" } ?? "user"),
                            "entries": .array([.object(["path": "public/rules.md", "name": "rules.md",
                                                        "folder": false, "size": 7, "mode": "rw"])])])
        case "volume_read":
            return .object(["path": .string(arguments["path"]?.stringValue ?? ""), "etag": "e1",
                            "version": "v1", "encoding": "utf8", "content": "be kind"])
        case "volume_write", "volume_restore":
            return .object(["key": .string(arguments["path"]?.stringValue ?? ""), "size": 7,
                            "etag": "e2", "version": "v2", "modified_ms": 1])
        case "volume_delete":
            return .object(["deleted": .string(arguments["path"]?.stringValue ?? "")])
        case "volume_history":
            return .object(["path": .string(arguments["path"]?.stringValue ?? ""),
                            "versions": .array([.object(["version": "v2", "latest": true]),
                                                .object(["version": "v1"])])])
        case "volume_grant", "volume_revoke", "volume_approve":
            return .object(["id": "g-1", "principal": "agent:ada", "prefix": "agents/bob/",
                            "mode": "r", "revoked": .bool(name == "volume_revoke")])
        case "volume_grants":
            return .object(["grants": .array([.object(["id": "g-1", "principal": "agent:ada",
                                                       "prefix": "agents/bob/", "mode": "r"])])])
        case "volume_request_access":
            return .object(["id": "r-1", "principal": .string("agent:\(arguments["as_agent"]?.stringValue ?? "")"),
                            "prefix": .string(arguments["prefix"]?.stringValue ?? ""),
                            "mode": .string(arguments["mode"]?.stringValue ?? "r"),
                            "reason": .string(arguments["reason"]?.stringValue ?? "")])
        case "volume_requests":
            return .object(["requests": .array([])])
        case "volume_deny":
            return .object(["denied": .string(arguments["request_id"]?.stringValue ?? "")])
        case "volume_audit":
            return .object(["events": .array([.object(["seq": 1, "principal": "user",
                                                       "action": "grant", "path": "agents/bob/"])]),
                            "verified": true])

        case "volume_storage":
            return .object(["backend": "fs", "fs_path": "/h/drive/data", "has_keys": false,
                            "cloud_available": false])
        case "volume_storage_set":
            return .object(["ok": true, "reachable": true, "authorized": true, "versioning": true,
                            "applied": .bool(arguments["dry_run"]?.boolValue != true)])
        case "volume_mount_status", "volume_mount", "volume_unmount":
            let on = name == "volume_mount"
            return .object(["enabled": .bool(on), "state": .string(on ? "mounted" : "off"),
                            "method": "nfs", "volume_name": "Cua Volume",
                            "path": on ? .string("/Users/u/Cua Volume") : .null])
        case "volume_sync_status":
            return .object(["device_id": "d1", "device_name": "mac", "feed": "live",
                            "pending_uploads": 0, "pending_bytes": 0,
                            "conflicts": .array([.object(["path": "public/a.md",
                                                          "conflict_path": "public/a (conflict from B).md"])]),
                            "devices": .array([.object(["id": "d1", "name": "mac", "this_device": true])])])
        case "volume_sync_events":
            return .object(["events": .array([.object(["seq": 1, "kind": "remote_change",
                                                       "path": "public/a.md", "device": "d2"])]),
                            "next_seq": 1])
        case "volume_sync_resolve":
            return .object(["resolved": .string(arguments["path"]?.stringValue ?? "")])
        case "volume_cache_stats", "volume_cache_set", "volume_cache_clear":
            return .object(["size_bytes": 0, "capacity_bytes": arguments["capacity_bytes"] ?? 10737418240,
                            "blocks": 0, "hits": 0, "misses": 0, "hit_rate": 0])

        // --- Sharing through the relay ---------------------------------------
        case "share_space", "unshare_space", "space_shares":
            let shares: [JSONValue] = name == "share_space"
                ? [.object(["who": arguments["who"] ?? "bob@example.com",
                            "role": arguments["role"] ?? "viewer"])] : []
            return .object(["space": arguments["space"] ?? "local:cua-space-test",
                            "machine": "0123", "shares": .array(shares)])
        case "relay_register_space":
            return .object(["space": arguments["space"] ?? "local:cua-space-test",
                            "relay_space": "relay:0123", "machine": "0123", "online": true])
        case "relay_unregister_space":
            return .object(["space": arguments["space"] ?? "local:cua-space-test", "unregistered": true])

        // --- Your cloud ------------------------------------------------------
        case "cloud_status", "cloud_connect":
            let aws: [String: JSONValue] = [
                "name": "aws", "title": "AWS", "tier": "vm", "connected": true, "default": true,
                "credentials": .object(["found": true, "source": "~/.aws profile default"]),
                "region": "us-west-2", "label": "AWS \u{00B7} us-west-2", "ttl_hours": 8,
                "kinds": .array([.object(["image": "linux", "kind": "container", "supported": true,
                                          "machine_type": "t3.medium", "usd_per_hour": .number(0.05)])]),
            ]
            if name == "cloud_connect" {
                var row = aws
                row["checks"] = .array([.object(["name": "credentials", "ok": true, "detail": "account 1"])])
                return .object(row)
            }
            return .object(["default_on": "aws", "providers": .array([.object(aws)]),
                            "resources": .array([.object(["provider": "aws", "id": "i-1",
                                                          "type": "instance", "sandbox": "aws:space-1", "machine": "cloud-1",
                                                          "state": "running", "expired": false])])])
        case "cloud_test":
            return .object(["provider": arguments["provider"] ?? "aws", "ok": true, "account": "1",
                            "checks": .array([.object(["name": "credentials", "ok": true, "detail": "account 1"])])])
        case "cloud_disconnect":
            return .object(["provider": arguments["provider"] ?? "aws", "disconnected": true, "left": .array([])])
        case "cloud_sweep":
            return .object(["dry_run": arguments["dry_run"] ?? true,
                            "resources": .array([.object(["provider": "aws", "id": "i-9", "type": "instance",
                                                          "action": "delete", "reason": "expired"])])])

        default:
            throw SpacesError.toolFailed(tool: name, message: "error: unknown tool \(name)")
        }
    }

    /// Just enough shell to make cleanup and existence checks real.
    /// The fake is a Space's shell, so it expands `~` the way one does. A fake
    /// that did not is exactly how a quoted tilde went unnoticed: inside single
    /// quotes `test -e '~/x'` asks about a directory literally named `~`.
    static func expand(_ path: String) -> String {
        path.hasPrefix("~/") ? "/Users/lume" + String(path.dropFirst(1)) : path
    }

    private func runBash(_ command: String) -> String {
        var output = ""
        for part in command.split(separator: ";") {
            let c = part.trimmingCharacters(in: .whitespaces)
            if c.hasPrefix("rm -rf ") || c.hasPrefix("rm -f ") {
                let target = Self.expand(c.split(separator: " ").dropFirst(2)
                    .joined(separator: " ")
                    .trimmingCharacters(in: CharacterSet(charactersIn: " '")))
                guard !target.isEmpty else { continue }
                files = files.filter { $0 != target && !$0.hasPrefix(target + "/") }
            } else if c.hasPrefix("mkdir -p") {
                let target = c.replacingOccurrences(of: "mkdir -p", with: "")
                    .trimmingCharacters(in: CharacterSet(charactersIn: " '"))
                files.insert(Self.expand(target))
            } else if c.hasPrefix("test -e") {
                let quoted = c.components(separatedBy: "'")
                let target = Self.expand(quoted.count > 1
                    ? quoted[1]
                    : c.replacingOccurrences(of: "test -e", with: "")
                        .trimmingCharacters(in: .whitespaces))
                let present = files.contains { $0 == target || $0.hasPrefix(target + "/") }
                if c.contains("printf") {
                    // A real shell runs the `&& printf … || printf …` the SDK
                    // writes, so the fake answers in the same vocabulary and
                    // the nonce round trip is exercised rather than stubbed.
                    let wanted = present ? "E" : "M"
                    output += quoted.first { $0.hasPrefix(wanted) } ?? wanted
                } else {
                    output += present ? "yes" : "no"
                }
            } else if c.hasPrefix("cat ") && c.contains("/pid") {
                output += "4242"
            } else if c.hasPrefix("ps -Ao") {
                output += "4242 1\n99 1\n"
            }
        }
        return output
    }
}
