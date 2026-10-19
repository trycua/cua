// docs: test="swift"
import Cua

print(try ["linux", "windows", "macos"].map { try canonicalImage(os: $0, version: nil) })
// ["ghcr.io/trycua/linux:24.04", "ghcr.io/trycua/windows:2022", "ghcr.io/trycua/macos:26"]
