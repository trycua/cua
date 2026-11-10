import Testing
@testable import CuaSpaces

@Suite("Cloud target")
struct CloudTargetTests {
    /// Hetzner's API token rides in the call's arguments, and nowhere else.
    @Test func theTokenIsAnArgumentButNeverPrinted() {
        let secret = "tok-0123456789abcdef"
        let target = CloudTarget("hetzner", region: "fsn1", token: secret)
        #expect(target.arguments["token"]?.stringValue == secret)
        #expect(target.arguments["provider"]?.stringValue == "hetzner")
        #expect(!"\(target)".contains(secret))
        #expect(!String(reflecting: target).contains(secret))
        #expect(!"\(target.description)".contains(secret))
    }

    @Test func otherCloudsAndEmptyTokensSendNoToken() {
        #expect(CloudTarget("aws", region: "us-west-2").arguments["token"] == nil)
        #expect(CloudTarget("hetzner", token: "").arguments["token"] == nil)
    }
}
