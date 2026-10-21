import assert from "node:assert/strict"
import { existsSync } from "node:fs"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"

// Every await of an async SDK call passes the same continuation callback to
// rust_future_poll. The Node runtime must build one trampoline for it, not one
// per poll: each trampoline owns a ThreadsafeFunction whose libuv async handle
// is never freed, and the host's event loop walks every one of them on each
// wakeup. A pending waitForExit polls many times a second, so a per-poll build
// slowed an Electron main process to a stall within a day.
//
// RSS is too noisy to assert at this scale. The runtime records the cached
// trampoline on the function under a hidden Symbol, so assert on that instead.

const testDirectory = path.dirname(fileURLToPath(import.meta.url))
const distribution = path.resolve(testDirectory, "../dist")
const available = existsSync(path.join(distribution, "native", "cua_driver_sdk-ffi.js"))

test(
  "async calls reuse one continuation trampoline across polls",
  { skip: !available, timeout: 30_000 },
  async () => {
    const { default: nativeModule } = await import(
      path.join(distribution, "native", "cua_driver_sdk-ffi.js")
    )
    const sdk = await import(path.join(distribution, "index.js"))
    const native = nativeModule()
    const continuations = new Set()
    for (const name of Object.keys(native)) {
      if (!name.includes("rust_future_poll")) continue
      const poll = native[name]
      native[name] = (future, continuation, handle) => {
        continuations.add(continuation)
        return poll(future, continuation, handle)
      }
    }

    const host = new sdk.EmbeddedCuaDriverHost(
      path.join(testDirectory, "missing cua-driver"),
      "com.example.test",
    )
    try {
      const markers = []
      for (let call = 0; call < 50; call += 1) {
        await host.waitForExit("missing-generation").catch(() => {})
        const [continuation] = continuations
        markers.push(Object.getOwnPropertySymbols(continuation).map((key) => continuation[key]))
      }
      assert.equal(continuations.size, 1, "the SDK passes one continuation function")
      assert.equal(markers[0].length, 1, "the first poll caches one trampoline")
      assert.equal(typeof markers[0][0], "bigint")
      assert.deepEqual(markers.at(-1), markers[0], "later polls reuse the cached trampoline")
    } finally {
      host.uniffiDestroy()
    }
  },
)
