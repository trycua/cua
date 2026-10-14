import Foundation
import Testing
@testable import CuaSpaces

/// Anything you can do in the Spaces MCP, you can do here by name.
///
/// `SpacesConnection.callTool` stays in the surface — it is how you reach
/// something new before the SDK models it — but it is not coverage. This suite
/// drives the **typed** call for every advertised tool and asserts the tool it
/// actually reached, so a tool that quietly loses its typed path fails here
/// rather than in a tutorial that has to fall back to the escape hatch.
@Suite(.serialized) final class ToolCoverageTests {

    private var backend: FakeSpacesBackend!
    private var connection: SpacesConnection!

    init() async throws {
        backend = FakeSpacesBackend()
        connection = SpacesConnection(transport: backend)
    }

    private func localSpace() async throws -> Space {
        try await connection.attach(to: "local:cua-space-test")
    }

    /// The contract every backend serves: `cua_spaces_contract::tools()`, as
    /// exported by the linked SDK (`spacesToolMethods()`), not a copy.
    static var advertisedTools: Set<String> { Set(SpacesConnection.contractTools) }

    @Test func testTheContractIsCompleteTools() {
        XCTAssertEqual(Self.advertisedTools.count, 86)
        // The Python server's `local_rcdp` is gone; `stream_endpoint` mints a
        // media ticket for any provider, and `send_file` is new.
        XCTAssertFalse(Self.advertisedTools.contains("local_rcdp"))
        for tool in ["stream_endpoint", "send_file", "add_space", "remove_space",
                     "create_space", "delete_space", "list_spaces"] {
            XCTAssertTrue(Self.advertisedTools.contains(tool), tool)
        }
        // One lifecycle vocabulary: create / delete, add / remove.
        for gone in ["claim_space", "release_space", "get_or_create_space",
                     "local_provision_space"] {
            XCTAssertFalse(Self.advertisedTools.contains(gone), gone)
        }
    }

    /// Every advertised tool, reached by a typed SDK call. Nothing in this test
    /// uses `callTool` for a tool the SDK is supposed to model.
    @Test func testEveryAdvertisedToolHasATypedPath() async throws {
        await backend.resetCalls()

        // --- Spaces: listing, attaching, creating, deleting -----------------
        _ = try await connection.spaces()                       // list_spaces
        let space = try await localSpace()
        let direct = try await connection.add(url: "10.0.0.5:3211", token: "t") // add_space
        try await connection.remove(direct.id)                  // remove_space
        let cloud = try await connection.createSpace(on: .cloud)  // create_space
        let localNew = try await connection.createSpace(on: .local, name: "demo", reuse: true)
        try await localNew.stop()                               // stop_space
        try await localNew.start()                              // start_space
        try await localNew.delete()                             // delete_space
        try await connection.deleteSpace(cloud.id)

        // --- Shell and files ------------------------------------------------
        _ = try await space.bash("echo hi")                     // space_bash
        _ = try await space.write("hello", to: "~/note.txt")    // space_write
        let file = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("coverage-\(UUID().uuidString).txt")
        try "x".write(to: file, atomically: true, encoding: .utf8)
        defer { try? FileManager.default.removeItem(at: file) }
        _ = try await space.files.send(file)                    // upload
        _ = try await space.sendFile(file, intoDownloads: "in") // send_file
        _ = try await space.files.file("~/note.txt").url()      // download

        // --- Screen: in-app frames vs the operator's own desktop -----------
        _ = try await space.windows()                           // list_space_windows
        _ = try await space.streamEndpoint()                    // stream_endpoint
        try await space.operatorDisplay.pinPictureInPictureOnOperatorDesktop()
        try await space.operatorDisplay.unpinPictureInPictureFromOperatorDesktop()
        try await space.operatorDisplay.openViewerOnOperatorDesktop()
        try await space.operatorDisplay.streamWindowToOperatorDesktop("target-aaa")

        // --- The Space's own MCP services -----------------------------------
        _ = try await space.services.tools()                    // list_tools
        _ = try await space.services.call("screenshot")         // call_tool

        // --- Agents ----------------------------------------------------------
        let run = try await space.startAgent(prompt: "x")       // agent_start
        _ = try await run.status(tail: 0)                       // agent_status
        _ = try await run.send("hello")                         // agent_message
        let page = try await run.eventPage()                    // agent_events
        XCTAssertTrue(page.caughtUp)
        _ = try await run.interrupt()                           // agent_interrupt
        _ = try await space.runs()                              // agent_list
        _ = try await run.stop()                                // agent_stop
        _ = try await space.agents.harnessCapabilities()        // agent_capabilities

        // --- Session teleport -------------------------------------------------
        let chrome = TeleportableApp(id: "chrome", displayName: "Chrome",
                                     isInstalledOnHost: true)
        let manifest = try await space.sessions.manifest(for: chrome)  // teleport_manifest
        _ = try await space.sessions.send(
            try manifest.approving(manifest.entries, into: space,
                                   acknowledgingSensitiveItems: true)) // teleport_app

        // --- The Keyvault: sign in with a saved password ---------------------
        let pending = try await space.requestSiteLogin(url: "http://login.example.test:8000/",
                                                      agent: "ada") // request_site_login
        XCTAssertEqual(pending.status, .pending)
        let filled = try await space.requestSiteLogin(url: "http://login.example.test:8000/",
                                                     requestID: pending.requestID)
        XCTAssertEqual(filled.status, .filled)
        XCTAssertEqual(filled.usernameHint, "a***@example.test")

        // --- The host Mac's network -------------------------------------------
        _ = try await connection.hotspot.start(sharingWith: space.id)  // hotspot_start
        _ = try await connection.hotspot.status()                      // hotspot_status
        _ = try await connection.hotspot.stop()                        // hotspot_stop

        // --- Persistent agents, routines, notifications ------------------------
        let agents = await connection.persistentAgents
        _ = try await agents.create("ada", harness: "hermes", in: space.id)  // persistent_agent_create
        _ = try await agents.list()                                          // persistent_agent_list
        _ = try await agents.send("hello", to: "ada")                        // persistent_agent_send
        _ = try await agents.save("ada")                                     // persistent_agent_save
        _ = try await agents.pause("ada")                                    // agent_pause
        _ = try await agents.resume("ada")                                   // agent_resume
        let routine = try await agents.addRoutine(for: "ada", title: "Morning", prompt: "Plan",
                                                  schedule: .dailyAt(hour: 8, minute: 0)) // routine_add
        _ = try await agents.routines(of: "ada")                             // routine_list
        _ = try await agents.setRoutine(routine.id, enabled: false)          // routine_set_enabled
        try await agents.removeRoutine(routine.id)                           // routine_remove
        _ = try await agents.allowComputer(SpaceID("relay:0123"), for: "ada") // computer_access_grant
        _ = try await agents.computerAccess(for: "ada")                      // computer_access_list
        _ = try await agents.revokeComputer(for: "ada")                      // computer_access_revoke
        try await agents.remove("ada")                                       // persistent_agent_remove
        _ = try await connection.notifications.post("Your research is ready") // notify_user
        _ = try await connection.notifications.list()                        // notifications_list
        _ = try await connection.notifications.markRead()                    // notifications_ack

        // --- Sharing through the relay -----------------------------------------
        let shared = try await space.share(with: "bob@example.com", role: "editor") // share_space
        XCTAssertEqual(shared.people["bob@example.com"], "editor")
        _ = try await space.shares()                                         // space_shares
        _ = try await space.unshare("bob@example.com")                       // unshare_space
        let relayID = try await space.registerWithRelay()                   // relay_register_space
        XCTAssertEqual(relayID.rawValue, "relay:0123")
        try await space.unregisterFromRelay()                                // relay_unregister_space
        // --- Your cloud ---------------------------------------------------------
        _ = try await space.stop()                                           // space_stop
        _ = try await space.start()                                          // space_start
        let clouds = try await connection.cloud.status()                     // cloud_status
        XCTAssertEqual(clouds.providers.first?.label, "AWS \u{00B7} us-west-2")
        XCTAssertEqual(clouds.providers.first?.offers.first?.machineType, "t3.medium")
        let tested = try await connection.cloud.test(CloudTarget("aws"))     // cloud_test
        XCTAssertTrue(tested.ok)
        let (aws, checks) = try await connection.cloud.connect(
            CloudTarget("aws", region: "us-west-2"), makeDefault: true)      // cloud_connect
        XCTAssertTrue(aws.isDefault && checks.checks.count == 1)
        let swept = try await connection.cloud.sweep()                       // cloud_sweep
        XCTAssertEqual(swept.first?.action, "delete")
        try await connection.cloud.disconnect("aws")                         // cloud_disconnect
        // --- Cua Volume -----------------------------------------------------------
        let ada = DriveView.agent("ada", inSpace: space.id)
        _ = try await connection.drive.list("public/", as: ada)            // volume_ls
        let rules = try await connection.drive.read("public/rules.md")   // volume_read
        XCTAssertEqual(String(decoding: rules.content, as: UTF8.self), "be kind")
        try await connection.drive.write(Data("x".utf8), to: "agents/ada/m.md",
                                         ifEtag: rules.etag, as: ada)     // volume_write
        let history = try await connection.drive.history("agents/ada/m.md") // volume_history
        try await connection.drive.restore("agents/ada/m.md",
                                           version: history[1].version)  // volume_restore
        try await connection.drive.delete("agents/ada/m.md")             // volume_delete
        let asked = try await connection.drive.requestAccess(
            for: "ada", "r", on: "agents/bob/", reason: "cite")          // volume_request_access
        XCTAssertEqual(asked.principal, "agent:ada")
        _ = try await connection.drive.requests()                         // volume_requests
        _ = try await connection.drive.approve(asked.id)                  // volume_approve
        try await connection.drive.deny("r-2")                            // volume_deny
        let grant = try await connection.drive.grant("agent:ada", "r", on: "agents/bob/") // volume_grant
        _ = try await connection.drive.grants()                           // volume_grants
        _ = try await connection.drive.revoke(grant.id)                   // volume_revoke
        let audit = try await connection.drive.audit()                    // volume_audit
        XCTAssertTrue(audit.verified)
        let storage = try await connection.drive.storage()               // volume_storage
        XCTAssertEqual(storage.backend, "fs")
        let check = try await connection.drive.setStorage(backend: "fs", dryRun: true) // volume_storage_set
        XCTAssertTrue(check.ok && !check.applied)
        _ = try await connection.drive.mountStatus()                     // volume_mount_status
        let mounted = try await connection.drive.mount()                 // volume_mount
        XCTAssertEqual(mounted.state, "mounted")
        let unmounted = try await connection.drive.unmount()             // volume_unmount
        XCTAssertEqual(unmounted.state, "off")
        let sync = try await connection.drive.syncStatus()               // volume_sync_status
        XCTAssertEqual(sync.conflicts.count, 1)
        let events = try await connection.drive.syncEvents()             // volume_sync_events
        XCTAssertEqual(events.nextSeq, 1)
        try await connection.drive.resolveConflict("public/a.md")        // volume_sync_resolve
        _ = try await connection.drive.cacheStats()                      // volume_cache_stats
        let capped = try await connection.drive.setCacheCapacity(1 << 30) // volume_cache_set
        XCTAssertEqual(capped.capacityBytes, 1 << 30)
        _ = try await connection.drive.clearCache()                      // volume_cache_clear

        let reached = Set(await backend.calls)
        let missing = Self.advertisedTools.subtracting(reached)
        XCTAssertTrue(missing.isEmpty,
                      "tools with no typed path, reachable only via callTool: "
                      + missing.sorted().joined(separator: ", "))
    }

    // MARK: - In-space MCP services

    /// A service that is reachable but advertises nothing **exists**. Unity
    /// lists no tools until an Editor has a project open, and an empty array
    /// reads as "no such service" — which is the exact misreading the server
    /// warns about and the SDK must not flatten.
    @Test func testAReachableServiceWithNoToolsIsNotAMissingService() async throws {
        let space = try await localSpace()
        let unity = try await space.services.tools(of: "unity")
        XCTAssertTrue(unity.tools.isEmpty)
        XCTAssertNotNil(unity.notReadyWarning)
        XCTAssertTrue(unity.isEmptyButReachable,
                      "an empty service must be distinguishable from an absent one")
    }

    /// A caller has no way to guess that `blender` is a thing it could ask for,
    /// so the other services travel with every answer.
    @Test func testEveryListingAdvertisesTheSpacesOtherServices() async throws {
        let space = try await localSpace()
        let catalog = try await space.services.tools()
        XCTAssertEqual(catalog.service, .computer)
        XCTAssertTrue(catalog.otherServices.contains("blender"))
        let all = try await space.services.list()
        XCTAssertEqual(Set(all.map(\.name)),
                       ["computer-server", "blender", "unity"])
    }

    /// cua-driver answers with an image *and* text, so a tool result is parts,
    /// not a string.
    @Test func testAnInSpaceToolResultKeepsItsImageAndItsText() async throws {
        let space = try await localSpace()
        let parts = try await space.services.call("screenshot", on: .computer)
        XCTAssertEqual(parts.count, 2)
        XCTAssertTrue(parts.contains { if case .image = $0 { return true } else { return false } })
        XCTAssertEqual(ToolContent.text(of: parts), "ran screenshot")
    }

    /// Asking for a name gets full input schemas; a bare listing does not.
    @Test func testAFilteredListingCarriesInputSchemas() async throws {
        let space = try await localSpace()
        let bare = try await space.services.tools()
        XCTAssertNil(bare.tools.first?.inputSchema)
        let filtered = try await space.services.tools(matching: "screen")
        XCTAssertNotNil(filtered.tools.first?.inputSchema)
    }

    // MARK: - Hotspot

    @Test func testHotspotSharingIsScopedAndStopsOnAThrow() async throws {
        let space = try await localSpace()
        struct Boom: Error {}
        do {
            try await connection.hotspot.sharing(with: space.id) {
                let during = try await connection.hotspot.status()
                XCTAssertTrue(during.isSharing)
                XCTAssertEqual(during.space, space.id)
                throw Boom()
            }
            XCTFail("the body threw")
        } catch is Boom {}
        let after = try await connection.hotspot.status()
        XCTAssertFalse(after.isSharing,
                       "a hotspot left on is someone's Mac still routing a sandbox's traffic")
    }

    // MARK: - Operator display is not your app's UI

    /// The three display tools draw on the **operator's** machine. Their SDK
    /// names say so, and none of them returns anything renderable — which is
    /// the whole point of keeping them apart from `space.screen`.
    @Test func testOperatorDisplayCallsReachTheDisplayToolsAndReturnNothing() async throws {
        let space = try await localSpace()
        await backend.resetCalls()
        try await space.operatorDisplay.pinPictureInPictureOnOperatorDesktop()
        try await space.operatorDisplay.openViewerOnOperatorDesktop()
        let calls = await backend.calls
        XCTAssertEqual(calls, ["show_space_pip", "open_space_viewer"])
    }

    // MARK: - space_write is not upload

    @Test func testWritingTextNeedsNoLocalFileAndNoShellQuoting() async throws {
        let space = try await localSpace()
        await backend.resetCalls()
        let written = try await space.write("a 'quoted' $string\nwith newlines",
                                            to: "~/notes/x.txt")
        let writeCalls = await backend.calls
        XCTAssertEqual(writeCalls, ["space_write"],
                       "space_write must not be implemented as bash")
        XCTAssertEqual(written.path, "/Users/lume/notes/x.txt")
        let exists = try await space.fileExists("~/notes/x.txt")
        XCTAssertTrue(exists)
    }

    // MARK: - Creating says what it costs

    /// `attach` cannot create. The one call that can takes where it runs.
    @Test func testOnlyTheNamedCreateCallCanMakeASpace() async throws {
        await backend.resetCalls()
        _ = try? await connection.attach(to: "local:cua-space-test")
        let attachCalls = await backend.calls
        XCTAssertFalse(attachCalls.contains("create_space"))

        let local = try await connection.createSpace(on: .local)
        XCTAssertEqual(local.provider, .local, "a local Space is the free one")
    }

    /// The escape hatch stays, and it is for tools the SDK does not model.
    @Test func testCallToolRemainsAvailable() async throws {
        let raw = try await connection.callTool("agent_capabilities", [:])
        XCTAssertNotNil(raw.objectValue?["statuses"])
    }
}
