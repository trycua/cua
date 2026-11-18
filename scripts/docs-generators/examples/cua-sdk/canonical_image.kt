// docs: test="kotlin"
import ai.cua.sdk.canonicalImage

println(listOf("linux", "windows", "macos").map { canonicalImage(it, null) })
// [ghcr.io/trycua/linux:24.04, ghcr.io/trycua/windows:2022, ghcr.io/trycua/macos:26]
