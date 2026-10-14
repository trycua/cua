// docs: test="swift"
import Cua

let cua = try Cua.embedded()
let sb = try await cua.sandboxes().create(options: SandboxCreateOptions(
    on: "local",  // or "cloud"
    image: try Image.linux(),
    name: "dev",
    waitFor: [ReadinessProbe(service: "env")]  // until its spacesd answers
))
let guest = try await sb.spacesd(probeTimeoutMs: nil)
let out = try await guest.sh(line: "uname -a", timeoutMs: nil)
print(String(decoding: out.stdout, as: UTF8.self))
try await sb.delete()
