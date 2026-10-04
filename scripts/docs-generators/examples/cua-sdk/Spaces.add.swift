// docs: test="swift"
import Cua

let spaces = try Cua.embedded().spaces()
let info = try await spaces.add(url: "http://10.0.0.5:3211", token: "TOKEN", name: "lab")  // a machine running cua-spacesd
let space = try await spaces.space(space: info.id)
let out = try await space.bash(command: "echo hi", timeoutMs: nil)
print(info.provider, out.stdout, out.exitCode ?? -1)
