// docs: test="swift"
import Cua

let guest = try await Cua.embedded().spacesd(url: "http://10.0.0.5:3211", token: "TOKEN")
let shot = try await guest.screenshot(options: nil)  // PNG by default
try await guest.click(x: Double(shot.width) / 2, y: Double(shot.height) / 2)
print(shot.width, shot.image.count)
