// Canonical images, tiers and Omarchy through the Swift binding (pure
// helpers: no registry read, no host effects).
import Foundation
import Testing

@testable import Cua

private func published(_ ref: String) -> Bool {
    let root = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent()
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
    let url = root.appendingPathComponent("images/sandbox-images.json")
    guard let data = try? Data(contentsOf: url),
        let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
        let images = json["images"] as? [[String: Any]],
        let entry = images.first(where: { $0["ref"] as? String == ref })
    else { return true }
    return entry["published"] as? Bool ?? true
}

private func expectRef(_ ref: String, _ make: () throws -> String) {
    if published(ref) {
        #expect((try? make()) == ref)
    } else {
        do {
            _ = try make()
            Issue.record("\(ref) should not resolve before it is published")
        } catch let CuaError.ImageNotPublished(message) {
            #expect(message.contains("not published yet"))
        } catch {
            Issue.record("unexpected \(error)")
        }
    }
}

@Test func tiersAndOmarchy() throws {
    #expect(try Image.linux(tier: "full") == "ghcr.io/trycua/linux:24.04"
        || ProcessInfo.processInfo.environment["CUA_IMAGE_LINUX"] != nil)
    #expect(try Image.macos("sequoia") == "ghcr.io/trycua/macos:15")
    expectRef("ghcr.io/trycua/linux:24.04-slim") { try Image.linux(tier: "slim") }
    expectRef("ghcr.io/trycua/macos:26-xcode") { try Image.macos(tier: "xcode") }
    expectRef("ghcr.io/trycua/omarchy:edge") { try Image.omarchy() }
    #expect(throws: CuaError.self) { try Image.linux(tier: "xcode") }
}
