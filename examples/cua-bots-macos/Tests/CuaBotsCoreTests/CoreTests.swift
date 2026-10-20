// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Testing
import Foundation
import SwiftUI
@testable import CuaBotsCore

struct DirectiveTests {
    @Test func testParsesAndStripsMarkers() {
        let raw = """
        [[status: Checking inbox]]
        I looked through your inbox.
        [[step: Read the migration docs]]
        [[ask: Send the invoice to Lina | $1,200, due Friday]]
        Two things need you.
        [[notify: Your research is ready | Three sources, one summary]]
        """
        let p = DirectiveParser.parse(raw)
        #expect(p.text == "I looked through your inbox.\nTwo things need you.")
        #expect(p.directives == [
            .status("Checking inbox"),
            .step("Read the migration docs"),
            .ask(action: "Send the invoice to Lina", detail: "$1,200, due Friday"),
            .notify(title: "Your research is ready", body: "Three sources, one summary"),
        ])
    }

    @Test func testUnknownMarkersStayAsText() {
        let p = DirectiveParser.parse("Array [[1, 2]] and [[weird: x]] stay.")
        #expect(p.text == "Array [[1, 2]] and [[weird: x]] stay.")
        #expect(p.directives.isEmpty)
    }

    @Test func testUnclosedMarkerIsText() {
        #expect(DirectiveParser.parse("a [[status: b").text == "a [[status: b")
    }

    @Test func testLoginOutputDone() {
        let p = DirectiveParser.parse("[[login: github.com]][[output: outputs/a.md]][[done: Research]]")
        #expect(p.directives == [.login(site: "github.com"), .output(path: "outputs/a.md"), .done(title: "Research")])
        #expect(p.text == "")
    }
}

struct ScheduleDirectiveTests {
    @Test func testParsesSchedules() {
        #expect(DirectiveParser.parseSchedule("daily 09:00") == .daily(hour: 9, minute: 0))
        #expect(DirectiveParser.parseSchedule("weekly Mon 08:30") == .weekly(weekday: 2, hour: 8, minute: 30))
        #expect(DirectiveParser.parseSchedule("every 30m") == .every(minutes: 30))
        #expect(DirectiveParser.parseSchedule("every 2h") == .every(minutes: 120))
        #expect(DirectiveParser.parseSchedule("daily 25:00") == nil)
        let p = DirectiveParser.parse("On it.\n[[schedule: daily 09:00 | Morning briefing | Summarize overnight email]]")
        #expect(p.directives == [.schedule(.daily(hour: 9, minute: 0), title: "Morning briefing", prompt: "Summarize overnight email")])
        #expect(p.text == "On it.")
    }
}

struct RuleTests {
    @Test func testClassifies() {
        #expect(ActionClass.classify("Buy two tickets to Belize") == .purchase)
        #expect(ActionClass.classify("Send the draft to the team") == .communicate)
        #expect(ActionClass.classify("Change the bank password") == .credentials)
        #expect(ActionClass.classify("Delete the old branch") == .delete)
        #expect(ActionClass.classify("Log in to github.com") == .signIn)
        #expect(ActionClass.classify("Summarize the report") == .readOnly)
        #expect(ActionClass.classify("Rearrange furniture") == .other)
    }

    @Test func testDefaultsAskBeforeBuyingAndHandOffPasswords() {
        let r = CustomRule.defaults
        #expect(RuleEngine.decide("Order the cake", rules: r).behavior == .askFirst)
        #expect(RuleEngine.decide("Transfer money to savings", rules: r).behavior == .handOff)
        #expect(RuleEngine.decide("Research flights", rules: r).behavior == .withoutAsking)
    }

    @Test func testCustomRuleWinsButLockedRuleCannotBeBeaten() {
        var r = CustomRule.defaults
        r.append(CustomRule(action: "Send the weekly update to the team", behavior: .withoutAsking))
        r.append(CustomRule(action: "password resets are fine", behavior: .withoutAsking))
        #expect(RuleEngine.decide("Send the weekly update to the team channel", rules: r).behavior == .withoutAsking)
        #expect(RuleEngine.decide("Send a message to Dan", rules: r).behavior == .askFirst)
        #expect(RuleEngine.decide("Change my password", rules: r).behavior == .handOff)
    }

    @Test func testPreApprovedNeedsTheUsersOwnAsk() {
        let r = [CustomRule(action: "Book dinner", behavior: .ifPreApproved, actionClass: .purchase)]
        #expect(RuleEngine.decide("Book dinner for two", rules: r).behavior == .askFirst)
        #expect(RuleEngine.decide("Book dinner for two", rules: r, preApproved: true).behavior == .withoutAsking)
    }

    @Test func testYamlAndInstructions() {
        let yaml = RuleEngine.yaml(CustomRule.defaults)
        #expect(yaml.contains("behavior: hand-off"))
        #expect(yaml.contains("locked: true"))
        let text = RuleEngine.instructions(CustomRule.defaults)
        #expect(text.contains("[[ask:"))
        #expect(text.contains("(cannot be changed)"))
    }
}

struct ModelTests {
    @Test func testAgentNamesMatchTheVolumeRules() {
        #expect(Bot.agentName(for: "Ada") == "ada")
        #expect(Bot.agentName(for: "Dr. Koala  Bear!") == "dr.-koala-bear")
        #expect(Bot.agentName(for: "  ") == "bot")
        #expect(Bot.agentName(for: String(repeating: "a", count: 90)).count == 63)
        let valid = Bot.agentName(for: "Émile Zola")
        #expect(valid.allSatisfy { $0.isLowercase || $0.isNumber || "._-".contains($0) })
    }

    @Test func testHandleAndComputerTitle() {
        let bot = Bot(name: "Ada", owner: "sam")
        #expect(bot.handle == "@sam-ada")
        #expect(bot.computerTitle == "Ada's computer")
        #expect(Bot(name: "Jules").computerTitle == "Jules' computer")
        #expect(bot.spaceName == "bot-ada")
    }

    @Test func testVolumeLayout() {
        #expect(VolumeLayout.memory("ada") == "agents/ada/memory/MEMORY.md")
        #expect(VolumeLayout.space("local:bot-ada") == "spaces/local-bot-ada/")
        #expect(VolumeLayout.instructions("ada", harness: .claudeCode) == "agents/ada/CLAUDE.md")
    }

    @Test func testSchedules() {
        var cal = Calendar(identifier: .gregorian)
        cal.timeZone = TimeZone(identifier: "UTC")!
        let now = ISO8601DateFormatter().date(from: "2026-09-29T10:00:00Z")!  // a Tuesday
        #expect(TaskSchedule.daily(hour: 9, minute: 0).nextFire(after: now, calendar: cal) == ISO8601DateFormatter().date(from: "2026-09-30T09:00:00Z"))
        #expect(TaskSchedule.weekly(weekday: 2, hour: 8, minute: 30).nextFire(after: now, calendar: cal) == ISO8601DateFormatter().date(from: "2026-10-05T08:30:00Z"))
        #expect(TaskSchedule.every(minutes: 30).nextFire(after: now, calendar: cal) == now.addingTimeInterval(1800))
    }
}

struct VolumeTests {
    @Test func testWritesReadsAndRefusesSecrets() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let volume = LocalVolume(root: root)
        try volume.writeText("agents/ada/memory/MEMORY.md", "Sam likes short answers.\n")
        #expect(try volume.readText("agents/ada/memory/MEMORY.md") == "Sam likes short answers.\n")
        #expect(try volume.walk("agents/ada/").map(\.path) == ["agents/ada/memory/MEMORY.md"])
        #expect(throws: VolumeError.secretDetected(path: "agents/ada/memory/MEMORY.md", line: 2)) {
            try volume.writeText("agents/ada/memory/MEMORY.md", "ok\nkey sk-ant-abcdefghijklmnopqrstuvwxyz0123\n")
        }
        #expect(throws: VolumeError.self) { try volume.writeText("agents/../x", "a") }
        try volume.delete("agents/ada/")
        #expect(try volume.ls("agents/") == [])
    }
}

// MARK: - The store, end to end on a scripted engine

@MainActor
final class ScriptedEngine: BotEngine {
    var sent: [String] = []
    var replies: [String] = []
    var paused = false
    var resetCount = 0
    private var queue: [EngineUpdate] = []
    private var turn: UInt32 = 0
    private var seq: UInt64 = 0

    func provision(_ bot: Bot, progress: @escaping @MainActor (EnginePhase) -> Void) async throws -> String {
        progress(.creatingSpace("Creating"))
        progress(.ready)
        return "local:\(bot.spaceName)"
    }

    func start(_ bot: Bot, volume: VolumeStore, prompt: String) async throws -> String {
        try await send(bot, text: prompt)
        return "run-1"
    }

    func send(_ bot: Bot, text: String) async throws {
        sent.append(text)
        turn += 1
        let reply = replies.isEmpty ? "Okay." : replies.removeFirst()
        queue.append(.tool(turn: turn, title: "Reading files"))
        queue.append(.assistant(turn: turn, text: reply))
        queue.append(.turnEnded(turn: turn))
    }

    func poll(_ bot: Bot, cursor: UInt64) async throws -> (updates: [EngineUpdate], cursor: UInt64) {
        let out = queue
        queue = []
        seq += UInt64(out.count)
        return (out, seq)
    }

    func checkpoint(_ bot: Bot, volume: VolumeStore) async throws {}
    func push(_ bot: Bot, volume: VolumeStore) async throws {}
    func interrupt(_ bot: Bot) async throws {}
    func pause(_ bot: Bot) async throws { paused = true }
    func resume(_ bot: Bot) async throws { paused = false }
    func reset(_ bot: Bot) async throws { resetCount += 1 }
}

@MainActor
@Suite(.serialized)
struct StoreTests {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent("cua-bots-tests-" + UUID().uuidString)

    func makeStore(_ engine: ScriptedEngine) -> BotStore {
        BotStore(volume: LocalVolume(root: root.appendingPathComponent("volume")),
                 dataDirectory: root.appendingPathComponent("data"), engine: engine)
    }

    @Test func testCreateWritesTheHomeAndIntroduces() async throws {
        let engine = ScriptedEngine()
        engine.replies = ["[[status: Settling in]]\nHi Sam, I'm Ada."]
        let store = makeStore(engine)
        let bot = await store.createBot(name: "Ada", avatar: AvatarConfig(color: .violet), harness: .claudeCode,
                                        placement: .local)
        #expect(bot.spaceID == "local:bot-ada")
        #expect(try store.volume.readText("agents/ada/CLAUDE.md") != nil)
        #expect(try store.volume.readText("agents/ada/rules.yaml") != nil)
        #expect(try store.volume.readText("agents/ada/memory/MEMORY.md") != nil)
        await store.pollOnce(bot.id)
        #expect(store.messages(for: bot.id).last?.text == "Hi Sam, I'm Ada.")
        #expect(store.bot(bot.id)?.status == "Settling in")
        #expect(engine.sent.first!.contains("Introduce yourself"))
    }

    @Test func testApprovalRoundTrip() async throws {
        let engine = ScriptedEngine()
        engine.replies = ["Hello.", "Found a backup baker.\n[[ask: Pay the $40 deposit | Sunday Cake Co, Saturday 11am]]",
                          "Paid.\n[[done: Book the cake tasting]]"]
        let store = makeStore(engine)
        let bot = await store.createBot(name: "Jojo", avatar: .default(for: "Jojo"), harness: .hermes, placement: .local)
        await store.pollOnce(bot.id)
        await store.send(bot.id, "Find a new cake vendor and book a tasting")
        await store.pollOnce(bot.id)
        let pending = store.pendingApprovals(for: bot.id)
        #expect(pending.count == 1)
        #expect(store.bot(bot.id)?.mood == .needsApproval)
        #expect(store.notifications.first?.kind == .approval)
        await store.decide(pending[0].id, approve: true)
        #expect(engine.sent.last!.hasPrefix("Approved: Pay the $40 deposit"))
        await store.pollOnce(bot.id)
        #expect(store.pendingApprovals(for: bot.id).isEmpty)
        if case .result(let title, _) = store.messages(for: bot.id).last?.kind {
            #expect(title == "Book the cake tasting")
        } else { Issue.record("expected a result card") }
    }

    @Test func testPauseRefusesWorkAndResumeRestores() async throws {
        let engine = ScriptedEngine()
        let store = makeStore(engine)
        let bot = await store.createBot(name: "Alfred", avatar: .default(for: "Alfred"), harness: .openclaw, placement: .local)
        await store.pollOnce(bot.id)
        let task = store.schedule(bot.id, title: "Morning briefing", prompt: "Summarize overnight email",
                                  schedule: .daily(hour: 9, minute: 0))
        await store.pause(bot.id)
        #expect(engine.paused)
        #expect(store.tasks(for: bot.id).first?.state == .paused)
        let fired = await store.fire(taskID: task.id, botID: bot.id)
        #expect(!(fired))
        let before = engine.sent.count
        await store.send(bot.id, "are you there?")
        #expect(engine.sent.count == before)
        await store.resume(bot.id)
        #expect(!(engine.paused))
        #expect(store.tasks(for: bot.id).first?.state == .scheduled)
        let firedAfter = await store.fire(taskID: task.id, botID: bot.id)
        #expect(firedAfter)
        #expect(engine.sent.last!.hasPrefix("[routine] Morning briefing"))
    }

    @Test func testResetDeletesTheHome() async throws {
        let engine = ScriptedEngine()
        let store = makeStore(engine)
        let bot = await store.createBot(name: "Todd", avatar: .default(for: "Todd"), harness: .hermes, placement: .local)
        await store.reset(bot.id)
        #expect(engine.resetCount == 1)
        #expect(store.bots.isEmpty)
        #expect(try store.volume.readText("agents/todd/memory/MEMORY.md") == nil)
    }

    @Test func testBotsComeBackFromTheVolume() async throws {
        let engine = ScriptedEngine()
        let store = makeStore(engine)
        _ = await store.createBot(name: "Felipe", avatar: AvatarConfig(color: .sky, eyes: .round, ears: .round),
                                  harness: .codex, placement: .local)
        let again = makeStore(ScriptedEngine())
        #expect(again.bots.map(\.name) == ["Felipe"])
        #expect(again.bots.first?.avatar.ears == .round)
        #expect(again.bots.first?.runID == "run-1")
    }

    @Test func testLockedRulesSurviveEdits() async throws {
        let store = makeStore(ScriptedEngine())
        let bot = await store.createBot(name: "Ada", avatar: .default(for: "Ada"), harness: .hermes, placement: .local)
        store.setRules(bot.id, [CustomRule(action: "Post in #launch", behavior: .withoutAsking)])
        let rules = store.rules(for: bot.id)
        #expect(rules.contains { $0.locked })
        #expect(try store.volume.readText("agents/ada/rules.yaml")!.contains("Post in #launch"))
    }
}

@MainActor
struct AvatarTests {
    /// Every combination draws: a face, nose and eyes inside the frame.
    @Test func testEveryConfigurationRenders() throws {
        let out = ProcessInfo.processInfo.environment["CUA_BOTS_RENDER_DIR"].map(URL.init(fileURLWithPath:))
        for color in BotColor.allCases {
            for ears in EarShape.allCases {
                for mood in BotMood.allCases {
                    let config = AvatarConfig(color: color, eyes: .star, ears: ears)
                    let view = KoalaAvatar(config, mood: mood, animated: false).frame(width: 96, height: 96)
                    let renderer = ImageRenderer(content: view)
                    renderer.scale = 2
                    let image = try #require(renderer.cgImage)
                    #expect(image.width == 192)
                    #expect(opaqueFraction(image) > 0.25, "\(color) \(ears) \(mood)")
                    if let out, mood == .idle || ears == .scalloped {
                        try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
                        try png(image).write(to: out.appendingPathComponent("\(color)-\(ears)-\(mood).png"))
                    }
                }
            }
        }
    }

    func opaqueFraction(_ image: CGImage) -> Double {
        let w = image.width, h = image.height
        var data = [UInt8](repeating: 0, count: w * h * 4)
        let ctx = CGContext(data: &data, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4,
                            space: CGColorSpaceCreateDeviceRGB(),
                            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
        ctx.draw(image, in: CGRect(x: 0, y: 0, width: w, height: h))
        var n = 0
        for i in stride(from: 3, to: data.count, by: 4) where data[i] > 0 { n += 1 }
        return Double(n) / Double(w * h)
    }

    func png(_ image: CGImage) throws -> Data {
        let data = NSMutableData()
        let dest = CGImageDestinationCreateWithData(data, "public.png" as CFString, 1, nil)!
        CGImageDestinationAddImage(dest, image, nil)
        CGImageDestinationFinalize(dest)
        return data as Data
    }
}
