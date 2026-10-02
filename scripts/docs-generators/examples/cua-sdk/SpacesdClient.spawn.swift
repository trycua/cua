// docs: test="swift"
import Cua
import Foundation

let guest = try await Cua.embedded().spacesd(url: "http://10.0.0.5:3211", token: "TOKEN")
let proc = try await guest.spawn(command: SpacesdCommand("cat", stdin: true))
try await proc.writeStdin(data: Data("xyz".utf8))
try await proc.closeStdin()
print(String(decoding: try await proc.wait().stdout, as: UTF8.self))
