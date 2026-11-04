// docs: test="swift"
import Cua

let sb = try await Cua.embedded().sandboxes().connectUrl(url: "http://10.0.0.5:3211", token: "wrong-token", name: nil)
do {
    _ = try await sb.spacesd(probeTimeoutMs: 5000)
} catch CuaError.Unauthenticated(let message) {  // one case; any other CuaError propagates
    print("rejected:", message)
}
