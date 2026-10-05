// docs: test="swift"
import Cua

let guest = try await Cua.embedded().spacesd(url: "http://10.0.0.5:3211", token: "TOKEN")  // or: try await sandbox.spacesd(probeTimeoutMs: nil)
let out = try await guest.run(command: SpacesdCommand("echo", ["hello"]))
print(out.exit.success, String(decoding: out.stdout, as: UTF8.self))
