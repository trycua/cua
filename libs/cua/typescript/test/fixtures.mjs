// Spawns libs/cua/target/*/cua-test-fixtures (loopback MockServer spacesd
// with a scripted media socket + fake Fleet API). Exits when stdin closes.
import { spawn } from "node:child_process"
import { existsSync } from "node:fs"
import { dirname, join, resolve } from "node:path"
import { createInterface } from "node:readline"
import { fileURLToPath } from "node:url"

export const cuaRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..")

export function binary(env, name) {
  if (process.env[env]) return process.env[env]
  const exe = process.platform === "win32" ? `${name}.exe` : name
  for (const profile of ["debug", "release"]) {
    const candidate = join(cuaRoot, "target", profile, exe)
    if (existsSync(candidate)) return candidate
  }
  return undefined
}

export async function startFixtures() {
  const path = binary("CUA_TEST_FIXTURES", "cua-test-fixtures")
  if (!path) return undefined
  const child = spawn(path, [], { stdio: ["pipe", "pipe", "inherit"] })
  const lines = createInterface({ input: child.stdout })
  const first = await new Promise((resolveLine, reject) => {
    lines.once("line", resolveLine)
    // A stale fixture (built from another commit) refuses to serve and says
    // so on stderr.
    child.once("exit", (code) =>
      reject(
        new Error(
          `cua-test-fixtures exited early (${code}); if it is stale, rebuild it with libs/cua/scripts/build-test-fixtures.sh`,
        ),
      ),
    )
  })
  return {
    ...JSON.parse(first),
    async stop() {
      child.stdin.end()
      await new Promise((r) => {
        const timer = setTimeout(() => {
          child.kill()
          r()
        }, 10_000)
        child.once("exit", () => {
          clearTimeout(timer)
          r()
        })
      })
    },
  }
}

export async function waitFor(what, cond, timeoutMs = 5000) {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    if (cond()) return
    await new Promise((r) => setTimeout(r, 20))
  }
  throw new Error(`timed out waiting for ${what}`)
}
