// docs: test="kotlin"
import ai.cua.sdk.imageAlias

println(listOf(imageAlias("macos:sequoia"), imageAlias("ubuntu:24.04")))
// [ghcr.io/trycua/macos:15, null]
