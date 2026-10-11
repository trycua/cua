import Foundation
import Testing
@testable import CuaSpaces

/// The routines JSON the TypeScript and Rust SDKs write, and the group chat
/// bounds, which the three OpenKoalaBots samples share.
@Suite(.serialized) final class RoutinesGroupsTests {

    /// What `@trycua/cua/spaces/routines` writes (fractional seconds) and what
    /// `cua_spaces::routines` writes (whole seconds) both load.
    @MainActor @Test func testRoutinesWrittenByTheOtherSDKsLoad() throws {
        let file = FileManager.default.temporaryDirectory
            .appendingPathComponent("routines-\(UUID().uuidString).json")
        defer { try? FileManager.default.removeItem(at: file) }
        let json = """
        [{"id":"A","botID":"b","title":"T","prompt":"p","schedule":{"kind":"everyMinutes","minutes":5},
          "isEnabled":true,"createdAt":"2026-09-25T09:30:00.123Z"},
         {"id":"B","botID":"b","title":"U","prompt":"q","schedule":{"kind":"weeklyOn","weekday":2,"hour":9,"minute":0},
          "isEnabled":false,"createdAt":"2026-09-25T09:30:00Z","lastFiredAt":"2026-09-25T10:00:00Z",
          "lastRunID":"run-1","lastOutcome":"started run run-1"}]
        """
        try json.write(to: file, atomically: true, encoding: .utf8)
        let store = RoutineStore(fileURL: file)
        XCTAssertEqual(store.routines.map(\.id), ["A", "B"])
        XCTAssertEqual(store.routine("B")?.schedule, .weeklyOn(weekday: 2, hour: 9, minute: 0))
        XCTAssertEqual(store.routine("B")?.lastRunID, "run-1")
        XCTAssertEqual(store.routine("A")?.schedule.label, "Every 5 minutes")
        XCTAssertEqual(store.routine("A")?.turnText, "[routine] T: p")
        XCTAssertTrue(store.log.isEmpty, "\(store.log)")
        // And this SDK writes whole seconds, the portable form.
        store.setEnabled(true, for: "B")
        let saved = try String(contentsOf: file, encoding: .utf8)
        XCTAssertFalse(saved.contains(".123"))
    }

    @MainActor @Test func testGroupBoundsHoldInTheSDK() throws {
        XCTAssertThrowsError(try GroupChat(title: "x", members: ["a"]))
        XCTAssertThrowsError(try GroupChat(title: "x", members: (0..<7).map { "b\($0)" }))
        let chat = try GroupChat(title: "Launch", members: ["ada", "bo"])
        XCTAssertEqual(chat.membershipLabel, "2 of 6 bots")
        let framed = GroupChatStore.frame("Status?", for: "ada", in: chat) { $0 == "bo" ? "Bo" : $0 }
        XCTAssertTrue(framed.hasPrefix("[group:Launch] You are in a group chat with the user and Bo."))
        XCTAssertEqual(framed.split(separator: "\n").last, "Status?")
    }
}
