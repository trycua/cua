// Headless driver for the page: serves it on loopback, opens it in
// Playwright's headless Chromium (ephemeral profile), and relays what the
// page reports: log lines, the SUMMARY line, WAV files and, in bench mode,
// the JSONL (plus the final `end` line with Chromium's CPU time).
//
// Environment: see examples/streaming/SCENARIO.md. Exit codes: the page's
// (0 ok, 1 failed), 2 bad config, 3 skipped (Playwright/Chromium/SDK
// browser build missing).
import { appendFileSync, existsSync, mkdirSync, writeFileSync } from "node:fs"
import { join } from "node:path"

import { sdkBrowserDir, serve } from "./serve.mjs"

process.env.PLAYWRIGHT_BROWSERS_PATH ??= "0" // browsers live in node_modules

const skip = (why) => {
  console.error(`skipped: ${why}`)
  process.exit(3)
}

let chromium
try {
  ;({ chromium } = await import("playwright"))
} catch {
  skip("playwright is not installed (npm install in examples/streaming/typescript-web)")
}
if (!existsSync(join(sdkBrowserDir, "index.js"))) {
  skip(`no @trycua/cua/browser build at ${sdkBrowserDir} (cd libs/cua/typescript && npm run build:browser)`)
}

const token = process.env.CUA_ENV_TOKEN
if (!token) {
  console.error("CUA_ENV_TOKEN is required")
  process.exit(2)
}
const benchPath = process.env.CUA_BENCH_JSONL
const outDir = process.env.CUA_OUT_DIR ?? "./out"
const seconds = Number(benchPath ? (process.env.CUA_BENCH_SECONDS ?? process.env.CUA_STREAM_SECONDS ?? 5) : (process.env.CUA_STREAM_SECONDS ?? 5))
const config = {
  env: process.env.CUA_ENV_URL ?? "http://127.0.0.1:33211",
  token,
  seconds,
  outDir,
  headless: true,
  bench: !!benchPath,
  benchTarget: process.env.CUA_BENCH_TARGET ?? "display:primary",
  benchAudio: (process.env.CUA_BENCH_AUDIO ?? "1") !== "0",
}

// Plain launch (not launchPersistentContext): a fresh temporary profile that
// Playwright deletes on close. Headless mode runs chromium-headless-shell.
let browser
try {
  browser = await chromium.launch({ headless: true })
} catch (e) {
  skip(`Playwright Chromium is not installed (npm run install-browser): ${String(e.message).split("\n")[0]}`)
}
const { server, url } = await serve(0)
let code = 1
try {
  const page = await browser.newPage()
  let done
  const finished = new Promise((ok) => (done = ok))
  await page.exposeFunction("cuaReport", (kind, data) => {
    if (kind === "log") console.log(data)
    else if (kind === "summary") console.log(`SUMMARY ${JSON.stringify(data)}`)
    else if (kind === "jsonl" && benchPath) appendFileSync(benchPath, data + "\n")
    else if (kind === "file") {
      mkdirSync(outDir, { recursive: true })
      writeFileSync(join(outDir, data.name), Buffer.from(data.base64, "base64"))
    } else if (kind === "done") done(data)
  })
  page.on("pageerror", (e) => console.error(`page error: ${e.message}`))
  page.on("console", (m) => m.type() === "error" && console.error(`console: ${m.text()}`))
  await page.addInitScript((c) => (window.__CUA_CONFIG = c), config)
  await page.goto(url)
  const budget = (benchPath ? seconds + 30 : seconds * 2 + 90) * 1000
  code = await Promise.race([
    finished,
    new Promise((ok) => setTimeout(() => (console.error(`timeout after ${budget} ms`), ok(1)), budget)),
  ])
  if (benchPath) {
    // Browser equivalent of getrusage: every Chromium process's CPU time
    // (browser, renderer, GPU, utility), user + system combined.
    let cpu = null
    try {
      const cdp = await browser.newBrowserCDPSession()
      const info = await cdp.send("SystemInfo.getProcessInfo")
      cpu = info.processInfo.reduce((s, p) => s + (p.cpuTime ?? 0), 0)
    } catch (e) {
      console.error(`cpu: ${e.message}`)
    }
    const ms = performance.timeOrigin + performance.now()
    const ns = BigInt(Math.floor(ms * 1000)) * 1000n
    appendFileSync(
      benchPath,
      `{"t":"end","unix_ns":${ns},"cpu_user_s":${cpu === null ? null : cpu.toFixed(3)},"cpu_sys_s":${cpu === null ? null : 0},"cpu_scope":"chromium processes, user+sys in cpu_user_s"}\n`,
    )
  }
} finally {
  await browser.close()
  server.close()
}
process.exit(code)
