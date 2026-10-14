// docs: test="swift"
import Cua

let cua = try Cua.embedded()
let sb = try await cua.sandboxes().connectUrl(url: "http://10.0.0.5:3211", token: "t", name: "dev")
let guest = try await sb.spacesd(probeTimeoutMs: nil)
let out = try await guest.run(command: SpacesdCommand("echo", ["hi"]))
