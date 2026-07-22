import assert from "node:assert/strict"
import { existsSync } from "node:fs"
import path from "node:path"
import { test } from "node:test"
import { fileURLToPath } from "node:url"

const testDirectory = path.dirname(fileURLToPath(import.meta.url))
const libraryName =
  process.platform === "darwin"
    ? "libcua_driver_sdk.dylib"
    : process.platform === "win32"
      ? "cua_driver_sdk.dll"
      : "libcua_driver_sdk.so"
const nodeTriple =
  process.platform === "darwin"
    ? `darwin-${process.arch}`
    : process.platform === "win32"
      ? `win32-${process.arch}-msvc`
      : `linux-${process.arch}-${process.report.getReport().header.glibcVersionRuntime ? "gnu" : "musl"}`
const library = path.resolve(
  testDirectory,
  "../node_modules/@trycua",
  `cua-driver-${nodeTriple}`,
  libraryName,
)

test(
  "embedded subpath is the same generated Rust host as the SDK root",
  { skip: !existsSync(library) },
  async () => {
    const root = await import("@trycua/cua-driver")
    const embedded = await import("@trycua/cua-driver/embedded")

    assert.equal(embedded.EmbeddedCuaDriverHost, root.EmbeddedCuaDriverHost)
    assert.equal(embedded.EmbeddedDriverHostOptions, root.EmbeddedDriverHostOptions)
    assert.equal(embedded.EmbeddedPermissionMode, root.EmbeddedPermissionMode)
    assert.throws(
      () => new embedded.EmbeddedCuaDriverHost("", "com.example.host"),
      error => error?.inner?.reason === "binary_path must not be empty",
    )

    assert.throws(
      () =>
        embedded.EmbeddedCuaDriverHost.withOptions(
          embedded.EmbeddedDriverHostOptions.new({
            binaryPath: "/example/cua-driver",
            hostBundleId: "com.example.host",
            approveSessionPolicy: false,
            dangerouslyBypassApprovals: false,
            environment: [
              embedded.EmbeddedEnvironmentVariable.new({
                name: "CUA_DRIVER_PERMISSION_MODE",
                value: "unrestricted",
              }),
            ],
            inheritStderr: false,
          }),
        ),
      error => error?.inner?.reason?.includes("safe allowlist") === true,
    )
  },
)
