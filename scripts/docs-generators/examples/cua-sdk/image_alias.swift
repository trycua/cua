// docs: test="swift"
import Cua

print(imageAlias(name: "macos:sequoia") ?? "none", imageAlias(name: "ubuntu:24.04") ?? "none")
// ghcr.io/trycua/macos:15 none
