// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import Foundation
import Testing

/// The Space thread's methods (`thread.*`): each one's daemon tool and
/// arguments, as the web bridge's `ops/thread.ts` builds them, and a daemon
/// without the thread answering `unsupported` (the page keeps its classic layout).
@MainActor
struct ThreadBridgeTests {
    @Test func readMapsTheCursorAndTheLongPoll() throws {
        let (tool, args) = try WebUIBridge.threadToolCall(
            "thread.read", ["spaceId": "local:dev", "after": 41, "limit": 100, "markRead": true, "followMs": 90_000])
        #expect(tool == "thread_read")
        #expect(args["space"] as? String == "local:dev")
        #expect((args["after"] as? NSNumber)?.intValue == 41)
        #expect((args["limit"] as? NSNumber)?.intValue == 100)
        #expect(args["mark_read"] as? Bool == true)
        // The tool's bound.
        #expect(args["follow_ms"] as? Int == 25_000)
    }

    @Test func readCarriesTheLiveKeyOnlyWithAFollow() throws {
        let (_, followed) = try WebUIBridge.threadToolCall(
            "thread.read", ["spaceId": "local:dev", "after": 41, "followMs": 25_000, "liveAfter": "claude=thinking"])
        #expect(followed["live_after"] as? String == "claude=thinking")
        let (_, plain) = try WebUIBridge.threadToolCall("thread.read", ["spaceId": "local:dev", "after": 41, "liveAfter": "claude=thinking"])
        #expect(plain["live_after"] == nil)
    }

    @Test func readPagesBack() throws {
        let (_, args) = try WebUIBridge.threadToolCall("thread.read", ["spaceId": "local:dev", "before": 40, "limit": 100])
        #expect((args["before"] as? NSNumber)?.intValue == 40)
        #expect(args["after"] == nil)
    }

    @Test func readAndMembersCarryTheAgentsTabsDmArguments() throws {
        let (_, read) = try WebUIBridge.threadToolCall("thread.read", ["spaceId": "local:dev", "with": "codex", "after": 3])
        #expect(read["with"] as? String == "codex")
        let (tool, members) = try WebUIBridge.threadToolCall("thread.members", ["spaceId": "local:dev", "overview": true])
        #expect(tool == "agent_members")
        #expect(members["overview"] as? Bool == true)
        let (_, plain) = try WebUIBridge.threadToolCall("thread.members", ["spaceId": "local:dev", "overview": false])
        #expect(Set(plain.keys) == ["space"])
    }

    @Test func readLeavesOutWhatIsNotSet() throws {
        let (_, args) = try WebUIBridge.threadToolCall("thread.read", ["spaceId": "local:dev", "markRead": false, "followMs": 0])
        #expect(Set(args.keys) == ["space"])
    }

    @Test func postCarriesTheAnswerToAnApproval() throws {
        let (tool, args) = try WebUIBridge.threadToolCall(
            "thread.post", ["spaceId": "local:dev", "text": "Approved", "answer": ["approval": 12, "approve": true]])
        #expect(tool == "thread_post")
        #expect(args["text"] as? String == "Approved")
        let answer = try #require(args["answer"] as? [String: Any])
        #expect((answer["approval"] as? NSNumber)?.intValue == 12)
        #expect(answer["approve"] as? Bool == true)
        #expect(answer["note"] == nil)
    }

    @Test func membersIsAgentMembers() throws {
        let (tool, args) = try WebUIBridge.threadToolCall("thread.members", ["spaceId": "relay:mac/dev"])
        #expect(tool == "agent_members")
        #expect(args["space"] as? String == "relay:mac/dev")
    }

    @Test func setRightsTakesTheNameAndOnlyTheRightsGiven() throws {
        let (tool, args) = try WebUIBridge.threadToolCall(
            "thread.setRights", ["spaceId": "relay:mac/dev", "name": "hermes", "task": "owner", "approve": ["acct_mia"]])
        #expect(tool == "agent_set_rights")
        #expect(args["name"] as? String == "hermes")
        #expect(args["task"] as? String == "owner")
        #expect(args["approve"] as? [String] == ["acct_mia"])
        #expect(Set(args.keys) == ["name", "task", "approve"])
        #expect(throws: WebUIBridge.Failure.self) { try WebUIBridge.threadToolCall("thread.setRights", ["spaceId": "relay:mac/dev"]) }
    }

    @Test func agentAddAndRemoveTakeTheName() throws {
        let (add, a) = try WebUIBridge.threadToolCall(
            "thread.agentAdd", ["spaceId": "relay:mac/dev", "name": "codex", "harness": "openai-codex"])
        #expect(add == "agent_add")
        #expect(a["space"] as? String == "relay:mac/dev")
        #expect(a["name"] as? String == "codex")
        #expect(a["agent"] as? String == "openai-codex")
        let (remove, r) = try WebUIBridge.threadToolCall("thread.agentRemove", ["spaceId": "relay:mac/dev", "name": "codex"])
        #expect(remove == "agent_remove")
        #expect(Set(r.keys) == ["name"])
        #expect(r["name"] as? String == "codex")
        #expect(throws: WebUIBridge.Failure.self) { try WebUIBridge.threadToolCall("thread.agentAdd", ["spaceId": "s", "name": "x"]) }
        #expect(throws: WebUIBridge.Failure.self) { try WebUIBridge.threadToolCall("thread.agentRemove", ["spaceId": "s"]) }
    }

    @Test func joinCodesMapTheirArguments() throws {
        let (create, c) = try WebUIBridge.threadToolCall(
            "thread.joinCodeCreate", ["spaceId": "local:dev", "ttlS": 900, "canTask": "members", "askBeforeJoin": true])
        #expect(create == "thread_join_code_create")
        #expect((c["ttl_s"] as? NSNumber)?.intValue == 900)
        #expect(c["can_task"] as? String == "members")
        #expect(c["ask_before_join"] as? Bool == true)
        let (_, bare) = try WebUIBridge.threadToolCall("thread.joinCodeCreate", ["spaceId": "local:dev"])
        #expect(Set(bare.keys) == ["space"])
        #expect(throws: WebUIBridge.Failure.self) {
            try WebUIBridge.threadToolCall("thread.joinCodeCreate", ["spaceId": "local:dev", "canTask": "everyone"])
        }
        let (list, l) = try WebUIBridge.threadToolCall("thread.joinCodeList", ["spaceId": "local:dev"])
        #expect(list == "thread_join_code_list")
        #expect(Set(l.keys) == ["space"])
        let (cancel, x) = try WebUIBridge.threadToolCall("thread.joinCodeCancel", ["spaceId": "local:dev", "id": "jc_1"])
        #expect(cancel == "thread_join_code_cancel")
        #expect(x["id"] as? String == "jc_1")
        #expect(throws: WebUIBridge.Failure.self) { try WebUIBridge.threadToolCall("thread.joinCodeCancel", ["spaceId": "local:dev"]) }
        let (settings, s) = try WebUIBridge.threadToolCall("thread.settings", ["spaceId": "local:dev", "askBeforeJoin": false])
        #expect(settings == "thread_settings")
        #expect(s["ask_before_join"] as? Bool == false)
        let (_, read) = try WebUIBridge.threadToolCall("thread.settings", ["spaceId": "local:dev"])
        #expect(Set(read.keys) == ["space"])
    }

    @Test func badArgumentsAreRefused() {
        #expect(throws: WebUIBridge.Failure.self) { try WebUIBridge.threadToolCall("thread.read", [:]) }
        #expect(throws: WebUIBridge.Failure.self) { try WebUIBridge.threadToolCall("thread.post", ["spaceId": "local:dev"]) }
        do {
            _ = try WebUIBridge.threadToolCall("thread.post", ["spaceId": "local:dev"])
        } catch let f as WebUIBridge.Failure {
            #expect(f.code == "bad_args")
        } catch {
            Issue.record("unexpected \(error)")
        }
    }

    @Test func aDaemonWithoutTheThreadIsUnsupported() {
        #expect(WebUIBridge.missingThreadTool("Spaces tool thread_read"))
        #expect(WebUIBridge.missingThreadTool("unknown tool agent_members"))
        // A Space that is not there is a real error, not an old daemon.
        #expect(!WebUIBridge.missingThreadTool("not_found: no Space local:gone"))
        #expect(!WebUIBridge.missingThreadTool("unknown tool volume_ls"))
    }
}
