// docs: test="swift"
import Cua
import Foundation

let guest = try await Cua.embedded().spacesd(url: "http://10.0.0.5:3211", token: "TOKEN")
let sent = try await guest.upload(path: "/tmp/note.txt", data: Data("hi from the host".utf8), options: nil)
let back = try await guest.download(path: "/tmp/note.txt")
print(sent.size, String(decoding: back, as: UTF8.self))
