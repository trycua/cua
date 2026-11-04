// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import Foundation
import Testing

/// The CI Runners methods (`ci.*`): each runs the bundled `cua ci app <op>`
/// with its JSON on stdin and answers the one JSON line it prints. Here a
/// fake `cua` stands in; `CiRunners` is the whole of what the host does.
struct CiRunnersTests {
    private func failure(_ body: () throws -> Any) -> WebUIBridge.Failure? {
        do { _ = try body() } catch let f as WebUIBridge.Failure { return f } catch { return nil }
        return nil
    }

    @Test func eachMethodRunsItsOwnOperationWithOnlyItsOwnKeys() throws {
        #expect(CiRunners.operations["ci.connectToken"] == "connect_token")
        #expect(Set(CiRunners.operations.keys) == ["ci.overview", "ci.connect", "ci.connectToken", "ci.connectCancel", "ci.repositories", "ci.goal", "ci.log"])
        #expect(try CiRunners.input(method: "ci.overview", args: ["live": true, "extra": "x"]) as NSDictionary == ["live": true])
        #expect(try CiRunners.input(method: "ci.overview", args: [:]) as NSDictionary == ["live": false])
        #expect(try CiRunners.input(method: "ci.connect", args: ["githubOrg": "acme", "other": 1]) as NSDictionary == ["githubOrg": "acme"])
        #expect(try CiRunners.input(method: "ci.connect", args: [:]).isEmpty)
        #expect(try CiRunners.input(method: "ci.connectToken", args: ["org": "acme", "token": "github_pat_x"]) as NSDictionary == ["org": "acme", "token": "github_pat_x"])
        #expect(try CiRunners.input(method: "ci.repositories", args: ["allow": ["a/b", 3, ""], "remove": "no"]) as NSDictionary == ["allow": ["a/b"], "remove": [String]()])
        #expect(try CiRunners.input(method: "ci.goal", args: ["target": "aws", "goal": "fastest", "x": 1]) as NSDictionary == ["target": "aws", "goal": "fastest"])
        #expect(try CiRunners.input(method: "ci.log", args: ["jobKey": "1/2", "pool": "hosted"]) as NSDictionary == ["jobKey": "1/2", "pool": "hosted", "slot": ""])
        #expect(CiRunners.arguments(operation: "overview") == ["ci", "app", "overview"])
    }

    @Test func missingArgumentsFailBeforeAnythingRuns() {
        #expect(failure { try CiRunners.input(method: "ci.connectToken", args: [:]) }?.code == "bad_args")
        #expect(failure { try CiRunners.input(method: "ci.goal", args: ["target": "aws"]) }?.code == "bad_args")
        #expect(failure { try CiRunners.input(method: "ci.log", args: ["jobKey": "1/2"]) }?.code == "bad_args")
    }

    @Test func theAnswerIsTheResultOfTheLastJsonLine() throws {
        let v = try CiRunners.answer(stdout: "noise\n{\"ok\":true,\"result\":{\"state\":\"waiting\",\"url\":\"https://github.com/x\"}}\n", stderr: "", status: 0)
        #expect((v as? [String: Any])?["state"] as? String == "waiting")
        #expect(try CiRunners.answer(stdout: "{\"ok\":true,\"result\":null}", stderr: "", status: 0) is NSNull)
    }

    @Test func aFailureKeepsItsWordsAndItsCodeIsMapped() {
        func f(_ code: String) -> WebUIBridge.Failure? {
            failure { try CiRunners.answer(stdout: "{\"ok\":false,\"error\":{\"code\":\"\(code)\",\"message\":\"words\"}}", stderr: "", status: 0) }
        }
        #expect(f("invalid")?.code == "bad_args")
        #expect(f("not_found")?.code == "not_found")
        #expect(f("unsupported")?.code == "unsupported")
        #expect(f("service")?.code == "failed")
        #expect(f("service")?.message == "words")
        let crashed = failure { try CiRunners.answer(stdout: "", stderr: "x\nerror: no home\n\n", status: 2) }
        #expect(crashed?.message == "error: no home")
        #expect(failure { try CiRunners.answer(stdout: "hello", stderr: "", status: 0) }?.message == "cua ci app answered something that is not JSON")
    }

    private func script(_ dir: URL, _ name: String, _ body: String) throws -> String {
        let file = dir.appendingPathComponent(name)
        try "#!/bin/sh\n\(body)\n".write(to: file, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: file.path)
        return file.path
    }

    @Test func runsCuaWithTheOperationAsArgumentsAndTheJsonOnStdinNeverTheToken() async throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-ci-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let argv = dir.appendingPathComponent("argv.txt").path, stdin = dir.appendingPathComponent("stdin.txt").path
        let cua = try script(dir, "cua", "printf '%s\\n' \"$@\" > \"\(argv)\"; cat > \"\(stdin)\"; echo '{\"ok\":true,\"result\":{\"state\":\"connected\"}}'")
        let run = await CiRunners.run(cua: cua, operation: "connect_token", input: ["org": "acme", "token": "github_pat_SECRET"])
        #expect(run.status == 0)
        let answer = try CiRunners.answer(stdout: run.stdout, stderr: run.stderr, status: run.status)
        #expect((answer as? [String: Any])?["state"] as? String == "connected")
        let args = try String(contentsOfFile: argv, encoding: .utf8)
        #expect(args.split(separator: "\n").map(String.init) == ["ci", "app", "connect_token"])
        #expect(!args.contains("SECRET"))
        let sent = try JSONSerialization.jsonObject(with: Data(contentsOf: URL(fileURLWithPath: stdin))) as? [String: Any]
        #expect(sent?["token"] as? String == "github_pat_SECRET")
        #expect(sent?["org"] as? String == "acme")
    }

    @Test func stopsARunThatTakesTooLongAndReportsACuaThatCannotStart() async throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-ci-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let slow = try script(dir, "slow", "sleep 5")
        let late = await CiRunners.run(cua: slow, operation: "overview", input: [:], timeout: 0.3)
        #expect(late.status == -1)
        #expect(late.stderr.contains("timed out"))
        let missing = await CiRunners.run(cua: dir.appendingPathComponent("missing").path, operation: "overview", input: [:])
        #expect(missing.status == -1)
        #expect(missing.stderr.contains("Could not start cua"))
    }
}
